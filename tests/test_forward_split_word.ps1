param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\psmux.exe')
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Exe)) { Write-Output "psmux binary not found: $Exe"; exit 2 }
$ExePath = (Resolve-Path -LiteralPath $Exe).Path

$TempBase = [IO.Path]::GetTempPath()
$Root = Join-Path $TempBase "psmux-forward-split-$PID"
Write-Output "temp=$TempBase"
Write-Output "root=$Root"
if (Test-Path -LiteralPath $Root) { Write-Output "root already exists, refusing: $Root"; exit 2 }

$FixedNames = 'USERPROFILE', 'HOME', 'TEMP', 'TMP', 'TMUX', 'TMUX_PANE', 'XDG_CONFIG_HOME', 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_MODE', 'PSMUX_PSREADLINE_HISTORY', 'PSMUX_NO_WARM'
$PsmuxNames = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' } | ForEach-Object { $_.Name })
$Original = @{}
foreach ($name in @($FixedNames + $PsmuxNames | Select-Object -Unique)) {
    $Original[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$Namespace = "fsw$PID"
$CR = [string][char]13
$script:Failures = 0
$script:Isolated = $false
$script:RootCreated = $false

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { Write-Output "PASS: $Name" } else { $script:Failures++; Write-Output "FAIL: $Name $Detail" }
}

function P {
    param([string[]]$Arguments)
    $info = [Diagnostics.ProcessStartInfo]::new($ExePath)
    foreach ($argument in @('-L', $Namespace) + $Arguments) { [void]$info.ArgumentList.Add($argument) }
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($info)
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(10000)) {
        try { $process.Kill() } catch {}
        $script:Failures++
        [Console]::Out.WriteLine("FAIL: timeout after 10s: $($Arguments -join ' ')")
        return [pscustomobject]@{ Code = -1; Text = '' }
    }
    $text = ''
    if ($out.Wait(2000)) { $text += $out.Result }
    if ($err.Wait(500)) { $text += $err.Result }
    [pscustomobject]@{ Code = $process.ExitCode; Text = $text -replace '\r?\n$', '' }
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

function Hex { param([string]$Text) ($Text.ToCharArray() | ForEach-Object { '{0:X2}' -f [int]$_ }) -join ' ' }

function Value { param([string]$Session, [string]$Name) (P @('show-options', '-v', '-t', $Session, $Name)).Text }

function Probe-Env { (P @('show-environment', '-t', 'B', 'PROBE')).Text }

function Registry-Value {
    param([string]$Session, [string]$Extension)
    $file = Get-ChildItem -LiteralPath $env:PSMUX_DATA_DIR -Recurse -File -Filter "$($Namespace)__$Session.$Extension" | Select-Object -First 1
    if ($null -eq $file) { throw "no registry file $($Namespace)__$Session.$Extension" }
    [IO.File]::ReadAllText($file.FullName).Trim()
}

function Send-Raw {
    param([string]$Session, [string]$Line)
    $port = [int](Registry-Value $Session 'port')
    $key = Registry-Value $Session 'key'
    $client = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    try {
        $stream = $client.GetStream()
        $bytes = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$Line`n")
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
        $stream.ReadTimeout = 1000
        $buffer = New-Object byte[] 4096
        $all = [IO.MemoryStream]::new()
        try { while (($count = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) { $all.Write($buffer, 0, $count) } } catch {}
        [Text.Encoding]::UTF8.GetString($all.ToArray())
    } finally {
        $client.Close()
    }
}

function Reset-B {
    $probe = P @('set-option', '-t', 'B', '@probe', 'none')
    $other = P @('set-option', '-t', 'B', '@other', 'none')
    $environment = P @('set-environment', '-t', 'B', 'PROBE', 'none')
    $fired = P @('set-option', '-t', 'A', '@fired', '0')
    if ($probe.Code -ne 0 -or $other.Code -ne 0 -or $environment.Code -ne 0 -or $fired.Code -ne 0) { throw 'could not reset the probe values' }
}

function Fire {
    param([string]$Row, [string]$Hook)
    if ($Hook) {
        $set = P @('set-hook', '-t', 'A', 'after-split-window', $Hook)
        if ($set.Code -ne 0) { throw "$Row could not set the hook: $($set.Text)" }
    }
    $mark = P @('set-hook', '-a', '-t', 'A', 'after-split-window', 'set-option -t A @fired 1')
    $split = P @('split-window', '-d', '-t', 'A')
    $fired = Wait-Until { (Value 'A' '@fired') -eq '1' }
    Check ($mark.Code -eq 0 -and $split.Code -eq 0 -and $fired) "$Row hook ran in A" "(set-hook -a $($mark.Code), split-window $($split.Code))"
    Start-Sleep -Milliseconds 1000
    [void](P @('set-hook', '-u', '-t', 'A', 'after-split-window'))
    [void](P @('kill-pane', '-a', '-t', 'A'))
}

function Check-B {
    param([string]$Row, [string]$Probe, [string]$Other = 'none', [string]$Environment = 'PROBE=none')
    $probeValue = Value 'B' '@probe'
    $otherValue = Value 'B' '@other'
    $environmentValue = Probe-Env
    Check ($probeValue -ceq $Probe) "$Row B @probe is [$Probe]" "(got [$probeValue])"
    Check ($otherValue -ceq $Other) "$Row B @other is [$Other]" "(got [$otherValue])"
    Check ($environmentValue -ceq $Environment) "$Row B has [$Environment]" "(got [$environmentValue])"
}

$HookRows = @(
    @{ Row = 'k1'; Hook = "set -t =B: @probe x ';' set @other v"; Probe = 'x ; set @other v' }
    @{ Row = 'k2'; Hook = "set -t B: @probe x ';' set @other v"; Probe = 'x ; set @other v' }
    @{ Row = 'k3'; Hook = "set -t =B: @probe ';'"; Probe = ';' }
    @{ Row = 'k4'; Hook = "set -t =B: @probe '\;'"; Probe = '\;' }
    @{ Row = 'k5'; Hook = "set -qt B: @probe x ';' set @other v"; Probe = 'x ; set @other v' }
    @{ Row = 'k6'; Hook = "setenv -t =B: PROBE x ';' set @other v"; Probe = 'none' }
    @{ Row = 'k7'; Hook = "set -t =B: @probe '\;' set @other v"; Probe = '\; set @other v' }
    @{ Row = 'e1'; Hook = "set -t =B: @probe '\\;' set @other v"; Probe = '\\; set @other v' }
    @{ Row = 'e2'; Hook = "set -t B: @probe '\\;' set @other v"; Probe = '\\; set @other v' }
    @{ Row = 'e3'; Hook = "set -t =B: @probe '\\\;' set @other v"; Probe = '\\\; set @other v' }
    @{ Row = 'e4'; Hook = "set -t =B: @probe '\\;'"; Probe = '\\;' }
    @{ Row = 'e5'; Hook = "set -t =B: @probe 'a\\;' set @other v"; Probe = 'a\\; set @other v' }
)

try {
    New-Item -ItemType Directory -Path $Root | Out-Null
    $script:RootCreated = $true
    foreach ($sub in 'data', 'home', 'tmp') { New-Item -ItemType Directory -Path (Join-Path $Root $sub) | Out-Null }
    $EmptyConfig = Join-Path $Root 'empty.conf'
    [IO.File]::WriteAllText($EmptyConfig, '')
    foreach ($name in $Original.Keys) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
    $env:PSMUX_DATA_DIR = Join-Path $Root 'data'
    $env:PSMUX_CONFIG_FILE = $EmptyConfig
    $env:PSMUX_PSREADLINE_HISTORY = Join-Path $Root 'history.txt'
    $env:PSMUX_NO_WARM = '1'
    $env:USERPROFILE = Join-Path $Root 'home'
    $env:HOME = Join-Path $Root 'home'
    $env:TEMP = Join-Path $Root 'tmp'
    $env:TMP = Join-Path $Root 'tmp'
    $leftover = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -notin 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_PSREADLINE_HISTORY', 'PSMUX_NO_WARM' })
    if ($leftover.Count -or -not (Test-Path -LiteralPath $env:PSMUX_DATA_DIR)) { throw "isolation is incomplete: $($leftover.Name -join ', ')" }
    $script:Isolated = $true

    foreach ($session in 'A', 'B') {
        $made = P @('new-session', '-d', '-s', $session, '-x', '120', '-y', '30')
        $up = Wait-Until { (P @('has-session', '-t', $session)).Code -eq 0 } 10000
        Check ($made.Code -eq 0 -and $up) "new-session $session is up" "(new-session exit $($made.Code))"
    }

    foreach ($row in $HookRows) {
        Reset-B
        Fire $row.Row $row.Hook
        Check-B $row.Row $row.Probe
    }

    Reset-B
    $reply = Send-Raw 'B' 'setenv -t B: PROBE x ";" set @other v'
    Check ($reply -clike "OK`n*ERROR: too many arguments (need at most 2)*") "w9 B rejects the quoted separator as an argument" "(reply [$($reply -replace "`n", '<LF>')])"
    Check-B 'w9' 'none'

    Reset-B
    $reply = Send-Raw 'A' ("set-hook -t A after-split-window 'set -t =B: @probe " + '"a' + $CR + 'b"' + "'")
    Check ($reply -clike "OK`n*") "r1 raw set-hook is accepted" "(reply [$($reply -replace "`n", '<LF>')])"
    Fire 'r1' ''
    $hex = Hex (Value 'B' '@probe')
    Check ($hex -ceq '61 5C 72 62') "r1 B @probe holds a, backslash, r, b" "(got [$hex])"
} catch {
    $script:Failures++
    Write-Output "FAIL: script error $($_.Exception.Message)"
} finally {
    if ($script:Isolated) {
        [void](P @('kill-server'))
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
