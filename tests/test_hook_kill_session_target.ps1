param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\psmux.exe'),
    [string]$EvidenceDir = ''
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Exe)) { Write-Output "psmux binary not found: $Exe"; exit 2 }
$ExePath = (Resolve-Path -LiteralPath $Exe).Path

$TempBase = [IO.Path]::GetTempPath()
$Root = Join-Path $TempBase "psmux-hook-kill-$PID"
Write-Output "temp=$TempBase"
Write-Output "root=$Root"
if (Test-Path -LiteralPath $Root) { Write-Output "root already exists, refusing: $Root"; exit 2 }

$FixedNames = 'USERPROFILE', 'HOME', 'TEMP', 'TMP', 'TMUX', 'TMUX_PANE', 'XDG_CONFIG_HOME', 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_MODE'
$PsmuxNames = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' } | ForEach-Object { $_.Name })
$Original = @{}
foreach ($name in @($FixedNames + $PsmuxNames | Select-Object -Unique)) {
    $Original[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$Namespaces = @("hks$PID", "hkh$PID")
$script:Failures = 0
$script:Isolated = $false
$script:RootCreated = $false

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { Write-Output "PASS: $Name" } else { $script:Failures++; Write-Output "FAIL: $Name $Detail" }
}

function P {
    param([string]$Namespace, [string[]]$Arguments)
    $output = & $ExePath -L $Namespace @Arguments 2>&1 | ForEach-Object { "$_" }
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = ($output -join "`n") }
}

function Wait-Until {
    param([scriptblock]$Condition, [int]$TimeoutMs = 5000)
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return [bool](& $Condition)
}

function Session-Alive { param([string]$Namespace, [string]$Session) (P $Namespace @('has-session', '-t', $Session)).Code -eq 0 }

function Listed-Sessions {
    param([string]$Namespace)
    $listing = P $Namespace @('ls')
    if ($listing.Code -ne 0) { return $null }
    , @($listing.Text -split "`n" | ForEach-Object { if ($_ -match '^([^:\s]+): ') { $Matches[1] } })
}

function Session-Gone {
    param([string]$Namespace, [string]$Session)
    $names = Listed-Sessions $Namespace
    $null -ne $names -and ($names -cnotcontains $Session) -and ($names -ccontains 'D')
}

function Read-Frame {
    param([string]$Namespace, [string]$Session)
    $dump = P $Namespace @('dump-state', '-t', $Session)
    if ($dump.Code -ne 0) { return $null }
    $script:LastFrameText = $dump.Text
    try { $frame = $dump.Text | ConvertFrom-Json -AsHashtable } catch { return $null }
    if ($frame -isnot [System.Collections.IDictionary]) { return $null }
    $frame
}

function Save-Frame {
    param([string]$Name)
    if ($EvidenceDir -and $null -ne $script:LastFrameText) {
        New-Item -ItemType Directory -Force $EvidenceDir | Out-Null
        [IO.File]::WriteAllText((Join-Path $EvidenceDir "$Name.json"), $script:LastFrameText)
    }
}

function New-TestSession {
    param([string]$Namespace, [string]$Session)
    $made = P $Namespace @('new-session', '-d', '-s', $Session)
    $up = Wait-Until { Session-Alive $Namespace $Session } 10000
    $timed = P $Namespace @('set-option', '-t', $Session, 'display-time', '8000')
    Check ($made.Code -eq 0 -and $up -and $timed.Code -eq 0) "new-session $Session is up" "(new-session exit $($made.Code), set-option exit $($timed.Code))"
}

function Check-DumpState {
    param([string]$Namespace, [string]$Session, [string]$Part)
    $script:LastFrameText = $null
    $frame = Read-Frame $Namespace $Session
    $name = if ($null -ne $frame) { $frame['session_name'] } else { $null }
    Save-Frame "$Part-$Session-first"
    Check ($null -ne $frame -and $name -ceq $Session) "dump-state -t $Session returns a JSON object whose session_name is $Session" "(session_name '$name')"
}

function Fire-Hook {
    param([string]$Namespace, [string]$Session, [string]$Command)
    $first = P $Namespace @('set-hook', '-t', $Session, 'after-split-window', $Command)
    $second = P $Namespace @('set-hook', '-a', '-t', $Session, 'after-split-window', 'set -g @fired 1')
    $split = P $Namespace @('split-window', '-t', $Session)
    Check ($first.Code -eq 0 -and $second.Code -eq 0 -and $split.Code -eq 0) "hooks set and split-window run on $Session" "(set-hook $($first.Code), set-hook -a $($second.Code), split-window $($split.Code))"
}

function Check-NotFound {
    param([string]$Namespace, [string]$Session, [string]$Expected, [string]$Part)
    $script:LastMessage = $null
    $script:LastFrameText = $null
    $shown = Wait-Until {
        $frame = Read-Frame $Namespace $Session
        $script:LastMessage = if ($null -ne $frame) { $frame['status_message'] } else { $null }
        $script:LastMessage -ceq $Expected
    }
    Save-Frame "$Part-$Session-status"
    Check $shown "$Session's status line is exactly '$Expected'" "(last: '$($script:LastMessage)')"
    Check (Session-Alive $Namespace $Session) "$Session is still alive after its hook"
    Check (Session-Alive $Namespace 'D') "D is still alive after $Session's hook"
}

# #938 (owner ruling 2026-10-31): a hook naming another session that exists
# runs on that session's server, as tmux and the CLI do, so D ends and the
# hook's own session lives.
function Check-OtherEnds {
    param([string]$Namespace, [string]$Session, [string]$Other)
    $ended = Wait-Until { -not (Session-Alive $Namespace $Other) }
    Check $ended "$Session's hook ends $Other"
    Check (Session-Alive $Namespace $Session) "$Session is still alive after its hook"
}

function Check-Ends {
    param([string]$Namespace, [string]$Session)
    $ended = Wait-Until { Session-Gone $Namespace $Session }
    Check $ended "$Session's own hook ends $Session (ls succeeds, lists D and not $Session)"
}

function Part-Default {
    param([string]$Namespace)
    Write-Output "Part 1: default mode"
    New-TestSession $Namespace 'A'
    New-TestSession $Namespace 'D'
    Check-DumpState $Namespace 'A' 'part1'
    Fire-Hook $Namespace 'A' 'kill-session -t D'
    Check-OtherEnds $Namespace 'A' 'D'
    $fired = Wait-Until {
        $shown = P $Namespace @('show-options', '-v', '-t', 'A', '@fired')
        $shown.Code -eq 0 -and $shown.Text.Trim() -eq '1'
    }
    Check $fired "A's next hook command ran (@fired is 1)"

    New-TestSession $Namespace 'D'
    New-TestSession $Namespace 'I'
    # tmux names the session part only (cmd-find.c:1258).
    Fire-Hook $Namespace 'I' 'kill-session -t Z:1.'
    Check-NotFound $Namespace 'I' "can't find session: Z" 'part1'

    New-TestSession $Namespace 'E'
    Fire-Hook $Namespace 'E' 'kill-session'
    Check-Ends $Namespace 'E'

    New-TestSession $Namespace 'F'
    Fire-Hook $Namespace 'F' 'kill-session -t F'
    Check-Ends $Namespace 'F'

    New-TestSession $Namespace 'G'
    Fire-Hook $Namespace 'G' 'kill-session -t =G'
    Check-Ends $Namespace 'G'

    New-TestSession $Namespace 'H'
    Fire-Hook $Namespace 'H' 'kill-session -a -t D'
    Check-Ends $Namespace 'H'
}

function Part-Shared {
    param([string]$Namespace)
    Write-Output "Part 2: shared mode"
    $env:PSMUX_SERVER_MODE = 'shared'
    try {
        New-TestSession $Namespace 'A'
        New-TestSession $Namespace 'D'
        Check-DumpState $Namespace 'A' 'part2'
        Fire-Hook $Namespace 'A' 'kill-session -t D'
        Check-OtherEnds $Namespace 'A' 'D'
    } finally {
        Remove-Item Env:PSMUX_SERVER_MODE -ErrorAction SilentlyContinue
    }
}

try {
    New-Item -ItemType Directory -Path $Root | Out-Null
    $script:RootCreated = $true
    foreach ($sub in 'data', 'home', 'tmp') { New-Item -ItemType Directory -Path (Join-Path $Root $sub) | Out-Null }
    $EmptyConfig = Join-Path $Root 'empty.conf'
    [IO.File]::WriteAllText($EmptyConfig, '')
    foreach ($name in $Original.Keys) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
    $env:PSMUX_DATA_DIR = Join-Path $Root 'data'
    $env:PSMUX_CONFIG_FILE = $EmptyConfig
    $env:USERPROFILE = Join-Path $Root 'home'
    $env:HOME = Join-Path $Root 'home'
    $env:TEMP = Join-Path $Root 'tmp'
    $env:TMP = Join-Path $Root 'tmp'
    $leftover = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -notin 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE' })
    if ($leftover.Count -or -not (Test-Path -LiteralPath $env:PSMUX_DATA_DIR)) { throw "isolation is incomplete: $($leftover.Name -join ', ')" }
    $script:Isolated = $true
    Part-Default $Namespaces[0]
    Part-Shared $Namespaces[1]
} catch {
    $script:Failures++
    Write-Output "FAIL: script error $($_.Exception.Message)"
} finally {
    if ($script:Isolated) {
        $env:PSMUX_SERVER_MODE = 'shared'
        P $Namespaces[1] @('kill-server') | Out-Null
        Remove-Item Env:PSMUX_SERVER_MODE -ErrorAction SilentlyContinue
        P $Namespaces[0] @('kill-server') | Out-Null
        $gone = Wait-Until { @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $ExePath }).Count -eq 0 } 10000
        Check $gone "no process of this binary is left after kill-server"
    }
    foreach ($name in $Original.Keys) {
        if ($null -eq $Original[$name]) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $Original[$name], 'Process') }
    }
    if ($script:RootCreated) {
        try {
            $links = @(Get-ChildItem -LiteralPath $Root -Recurse -Force -Attributes ReparsePoint -ErrorAction Stop)
            for ($dir = [IO.DirectoryInfo]::new($Root); $null -ne $dir; $dir = $dir.Parent) {
                if ($dir.Attributes -band [IO.FileAttributes]::ReparsePoint) { $links += $dir }
            }
            if ($links.Count) { throw "root, its path or its contents hold reparse points: $($links.FullName -join ', ')" }
            Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction Stop
        } catch {
            $script:Failures++
            Write-Output "FAIL: could not remove $Root ($($_.Exception.Message))"
        }
    }
}

if ($script:Failures -gt 0) { Write-Output "$($script:Failures) check(s) failed"; exit 1 }
exit 0
