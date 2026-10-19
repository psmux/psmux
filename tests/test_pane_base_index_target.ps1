# A numeric pane target under a non-zero pane-base-index.
#
# With `set -g pane-base-index 5` the panes of a window are numbered 5, 6, ...
# (#{pane_index}), and `-t X:.6` names the second pane. The server turned that
# number into a position without taking the base off, in the target check
# every -t command goes through, in display-message and in select-pane. So
# `-t X:.6` was "can't find pane: 6" (send-keys did nothing at exit 0), and
# `-t X:.1`, below the base, was read as position 1: its keys went into pane 6
# and select-pane moved there.
#
# Two windows, each with panes 5 and 6; window 0 and pane 5 are active.
# - Panes 5 and 6 of window 0 by @N.P, X:.P, X:<window>.P and X.P:
#   display-message prints that pane, send-keys reaches it only, select-pane
#   makes it active. %id and no -t at all are controls.
# - 1 (below the base) and 7 (after the last pane): display-message and
#   select-pane exit 1 with "can't find pane", no keys reach any pane, nothing
#   moves.
# - select-pane on window 1, which is not the current window: its active pane
#   becomes 6 and window 0 stays current; 1 is refused.
# - With pane-base-index set back to 0, the same targets one lower work.
#
# Isolation: a unique -L namespace, a PSMUX_DATA_DIR, USERPROFILE and HOME of
# its own, a config of one line, and cleanup only with `-L <ns> kill-server`.
# The default namespace's `ls` is compared before and after. Any mismatch,
# timeout, setup failure or cleanup failure exits non-zero.
#
# PSMUX_TEST_EXE picks the binary (default: psmux on PATH). PSMUX_TEST_ROOT
# picks where the scratch folder goes (default: the system temp folder).

$ErrorActionPreference = "Stop"
$script:Passed = 0
$script:Failed = 0
function Pass($Message) { Write-Host "  [PASS] $Message" -ForegroundColor Green; $script:Passed++ }
function Fail($Message) { Write-Host "  [FAIL] $Message" -ForegroundColor Red; $script:Failed++ }
function Check($Ok, $Message, $Detail) { if ($Ok) { Pass $Message } else { Fail "$Message -> $Detail" } }

$Psmux = $env:PSMUX_TEST_EXE
if (-not $Psmux) { $Found = Get-Command psmux -ErrorAction SilentlyContinue; if ($Found) { $Psmux = $Found.Source } }
if (-not $Psmux) { Write-Host "psmux binary not found"; exit 2 }
$Psmux = (Resolve-Path $Psmux).Path

$RootBase = if ($env:PSMUX_TEST_ROOT) { $env:PSMUX_TEST_ROOT } else { [System.IO.Path]::GetTempPath() }
$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$Root = Join-Path $RootBase ("psmux_pbi_" + $Tag)
$Ns = "pbi" + $Tag
$DataDir = Join-Path $Root "data"
$HomeDir = Join-Path $Root "home"
$ConfigFile = Join-Path $Root "pane-base.conf"

$TouchedNames = @('PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'USERPROFILE', 'HOME', 'TMUX', 'TMUX_PANE',
    'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL',
    'PSMUX_SERVER_MODE', 'PSMUX_REMOTE_ATTACH')
$SavedEnv = @{}
foreach ($Name in $TouchedNames) { $SavedEnv[$Name] = [Environment]::GetEnvironmentVariable($Name) }

function Restore-Env {
    foreach ($Name in $TouchedNames) {
        if ($null -eq $SavedEnv[$Name]) { Remove-Item "Env:$Name" -ErrorAction SilentlyContinue }
        else { Set-Item "Env:$Name" $SavedEnv[$Name] }
    }
}

function Get-DefaultLs {
    $Output = & $Psmux ls 2>&1 | ForEach-Object { "$_" }
    return ($Output -join "`n").Trim()
}

# Runs the binary in the test namespace with a time limit. A timeout kills
# only that process (by its own handle) and counts as a failure.
function Invoke-Psmux([string[]]$CliArgs, [int]$TimeoutMs = 15000) {
    $Info = New-Object System.Diagnostics.ProcessStartInfo
    $Info.FileName = $Psmux
    foreach ($Arg in @("-L", $Ns) + $CliArgs) { $Info.ArgumentList.Add($Arg) }
    $Info.UseShellExecute = $false
    $Info.RedirectStandardOutput = $true
    $Info.RedirectStandardError = $true
    $Process = [System.Diagnostics.Process]::Start($Info)
    $StdoutTask = $Process.StandardOutput.ReadToEndAsync()
    $StderrTask = $Process.StandardError.ReadToEndAsync()
    if (-not $Process.WaitForExit($TimeoutMs)) {
        $Process.Kill()
        $Process.WaitForExit()
        throw "timed out: $($CliArgs -join ' ')"
    }
    $Process.WaitForExit()
    return @{ Code = $Process.ExitCode; Out = $StdoutTask.Result.Trim(); Err = $StderrTask.Result.Trim() }
}

function Expect-Step([string[]]$CliArgs) {
    $Result = Invoke-Psmux $CliArgs
    if ($Result.Code -ne 0) { throw "step '$($CliArgs -join ' ')' failed: exit $($Result.Code), $($Result.Err)" }
    return $Result
}

# "<window id>|<pane index>|<pane active>|<window active>" for every pane.
function Get-Focus {
    return ((Expect-Step @("list-panes", "-a", "-F", "#{window_id}|#{pane_index}|#{pane_active}|#{window_active}")).Out -split "`r?`n" |
            Sort-Object) -join " "
}

function Reset-Focus {
    Expect-Step @("select-pane", "-t", $script:W1First.Id) | Out-Null
    Expect-Step @("select-window", "-t", $script:W0First.WinId) | Out-Null
    Expect-Step @("select-pane", "-t", $script:W0First.Id) | Out-Null
}

# Which panes show the marker text, by "<window id>.<pane index>".
function Get-MarkerPanes([string]$Marker) {
    $Found = @()
    foreach ($Pane in $script:Panes) {
        $Capture = Expect-Step @("capture-pane", "-t", $Pane.Id, "-p", "-J")
        if ($Capture.Out -match [regex]::Escape($Marker)) { $Found += "$($Pane.WinId).$($Pane.Index)" }
    }
    return ($Found -join ",")
}

$script:Number = 0
function Expect-Pane([string]$Target, $Pane) {
    $Display = Invoke-Psmux @("display-message", "-t", $Target, "-p", "#{pane_index}|#{pane_id}")
    Check ($Display.Code -eq 0 -and $Display.Out -eq "$($Pane.Index)|$($Pane.Id)") "display-message -t $Target is pane $($Pane.Index)" "exit $($Display.Code), '$($Display.Out)', '$($Display.Err)'"

    $script:Number++
    $Marker = "pbimark" + $script:Number.ToString("000") + "x"
    $Send = Invoke-Psmux @("send-keys", "-t", $Target, $Marker)
    Start-Sleep -Milliseconds 400
    $Landed = Get-MarkerPanes $Marker
    Check ($Send.Code -eq 0 -and $Landed -eq "$($Pane.WinId).$($Pane.Index)") "send-keys -t $Target reaches pane $($Pane.Index) only" "exit $($Send.Code), landed in '$Landed'"

    Reset-Focus
    $Select = Invoke-Psmux @("select-pane", "-t", $Target)
    $Focus = Get-Focus
    $Want = "$($Pane.WinId)|$($Pane.Index)|1|1"
    Check ($Select.Code -eq 0 -and $Focus -match [regex]::Escape($Want)) "select-pane -t $Target makes pane $($Pane.Index) active" "exit $($Select.Code), focus '$Focus'"
    Reset-Focus
}

function Expect-NoPane([string]$Target) {
    Reset-Focus
    $Before = Get-Focus
    $Display = Invoke-Psmux @("display-message", "-t", $Target, "-p", "#{pane_index}|#{pane_id}")
    Check ($Display.Code -eq 1 -and $Display.Out -eq "" -and $Display.Err -match "can't find pane") "display-message -t $Target is refused" "exit $($Display.Code), '$($Display.Out)', '$($Display.Err)'"

    $script:Number++
    $Marker = "pbimark" + $script:Number.ToString("000") + "x"
    $Send = Invoke-Psmux @("send-keys", "-t", $Target, $Marker)
    Start-Sleep -Milliseconds 400
    $Landed = Get-MarkerPanes $Marker
    Check ($Landed -eq "") "send-keys -t $Target reaches no pane" "exit $($Send.Code), landed in '$Landed'"

    $Select = Invoke-Psmux @("select-pane", "-t", $Target)
    $After = Get-Focus
    Check ($Select.Code -eq 1 -and $Select.Err -match "can't find pane" -and $After -eq $Before) "select-pane -t $Target is refused and moves nothing" "exit $($Select.Code), '$($Select.Err)', focus '$After' (was '$Before')"
}

$DefaultBefore = Get-DefaultLs
$SetupOk = $false
$ServerPids = @()
try {
    New-Item -ItemType Directory -Force $DataDir, $HomeDir | Out-Null
    [System.IO.File]::WriteAllText($ConfigFile, "set -g pane-base-index 5`n")
    [System.IO.File]::WriteAllText((Join-Path $HomeDir ".psmux.conf"), "")
    [System.IO.File]::WriteAllText((Join-Path $HomeDir ".tmux.conf"), "")
    foreach ($Name in $TouchedNames) { Remove-Item "Env:$Name" -ErrorAction SilentlyContinue }
    $env:PSMUX_DATA_DIR = $DataDir
    $env:PSMUX_CONFIG_FILE = $ConfigFile
    $env:USERPROFILE = $HomeDir
    $env:HOME = $HomeDir

    Write-Host "`n=== numeric pane targets under pane-base-index ===" -ForegroundColor Cyan
    Write-Host "  Binary:    $Psmux"
    Write-Host "  Namespace: $Ns"

    & $Psmux -L $Ns new-session -d -s X *> $null
    $Deadline = (Get-Date).AddSeconds(15)
    do {
        $Probe = Invoke-Psmux @("has-session", "-t", "X") 5000
        if ($Probe.Code -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $Deadline)
    foreach ($PidFile in Get-ChildItem $DataDir -Filter "*.pid" -ErrorAction SilentlyContinue) {
        $ServerPids += [int](([System.IO.File]::ReadAllText($PidFile.FullName)).Trim().Split(":")[0])
    }
    if ($Probe.Code -ne 0) { throw "session X did not come up" }
    Expect-Step @("split-window", "-t", "X:0") | Out-Null
    Expect-Step @("new-window", "-t", "X") | Out-Null
    Expect-Step @("split-window", "-t", "X:1") | Out-Null
    Start-Sleep -Milliseconds 1500
    $Rows = (Expect-Step @("list-panes", "-a", "-F", "#{window_index}|#{window_id}|#{pane_index}|#{pane_id}")).Out -split "`r?`n"
    $script:Panes = @($Rows | ForEach-Object { $Field = $_.Split("|"); [pscustomobject]@{ WinIdx = $Field[0]; WinId = $Field[1]; Index = $Field[2]; Id = $Field[3] } })
    $Layout = ($script:Panes | ForEach-Object { "$($_.WinIdx).$($_.Index)" }) -join ","
    if ($Layout -ne "0.5,0.6,1.5,1.6") { throw "unexpected layout '$Layout'" }
    $script:W0First = $script:Panes[0]; $W0Second = $script:Panes[1]
    $script:W1First = $script:Panes[2]; $W1Second = $script:Panes[3]
    $W0 = $script:W0First.WinId; $W1 = $script:W1First.WinId
    Reset-Focus
    if ((Get-Focus) -notmatch [regex]::Escape("$W0|5|1|1")) { throw "focus did not reset: $(Get-Focus)" }
    $SetupOk = $true
    Write-Host "  Windows:   $W0 (current) and $W1, panes 5 and 6 in each"

    Write-Host "`n--- pane-base-index 5: panes 5 and 6 of the current window ---" -ForegroundColor Yellow
    foreach ($Target in @("$W0.6", "X:.6", "X:0.6", "X.6", $W0Second.Id)) { Expect-Pane $Target $W0Second }
    foreach ($Target in @("$W0.5", "X:.5")) { Expect-Pane $Target $script:W0First }
    $NoTarget = Invoke-Psmux @("display-message", "-p", "#{pane_index}|#{pane_id}")
    Check ($NoTarget.Code -eq 0 -and $NoTarget.Out -eq "5|$($script:W0First.Id)") "display-message without -t is the active pane 5" "exit $($NoTarget.Code), '$($NoTarget.Out)'"

    Write-Host "`n--- pane-base-index 5: 1 and 7 name no pane ---" -ForegroundColor Yellow
    foreach ($Target in @("$W0.1", "X:.1", "$W0.7", "X:.7")) { Expect-NoPane $Target }

    Write-Host "`n--- pane-base-index 5: select-pane in the window that is not current ---" -ForegroundColor Yellow
    foreach ($Target in @("$W1.6", "X:1.6")) {
        Reset-Focus
        $Select = Invoke-Psmux @("select-pane", "-t", $Target)
        $Focus = Get-Focus
        Check ($Select.Code -eq 0 -and $Focus -match [regex]::Escape("$W1|6|1|0") -and $Focus -match [regex]::Escape("$W0|5|1|1")) "select-pane -t $Target moves window 1 to pane 6, window 0 stays current" "exit $($Select.Code), focus '$Focus'"
    }
    Reset-Focus
    $Before = Get-Focus
    $Select = Invoke-Psmux @("select-pane", "-t", "$W1.1")
    $After = Get-Focus
    Check ($Select.Code -eq 1 -and $Select.Err -match "can't find pane" -and $After -eq $Before) "select-pane -t $W1.1 is refused and moves nothing" "exit $($Select.Code), '$($Select.Err)', focus '$After' (was '$Before')"

    Write-Host "`n--- pane-base-index 0: the same panes one number lower ---" -ForegroundColor Yellow
    Expect-Step @("set-option", "-g", "pane-base-index", "0") | Out-Null
    foreach ($Pane in $script:Panes) { $Pane.Index = [string]([int]$Pane.Index - 5) }
    foreach ($Target in @("$W0.1", "X:.1", "X:0.1", "X.1")) { Expect-Pane $Target $W0Second }
    Expect-Pane "X:.0" $script:W0First
    Reset-Focus
    $Select = Invoke-Psmux @("select-pane", "-t", "$W1.1")
    $Focus = Get-Focus
    Check ($Select.Code -eq 0 -and $Focus -match [regex]::Escape("$W1|1|1|0") -and $Focus -match [regex]::Escape("$W0|0|1|1")) "select-pane -t $W1.1 moves window 1 to pane 1, window 0 stays current" "exit $($Select.Code), focus '$Focus'"
}
catch {
    Fail "setup or unexpected error: $_"
}
finally {
    $Kill = & $Psmux -L $Ns kill-server 2>&1
    $KillCode = $LASTEXITCODE
    $Alive = @()
    $Deadline = (Get-Date).AddSeconds(10)
    do {
        $Alive = @($ServerPids | Where-Object { $Proc = Get-Process -Id $_ -ErrorAction SilentlyContinue; $Proc -and $Proc.Path -eq $Psmux })
        if ($Alive.Count -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $Deadline)
    Restore-Env
    if ($ServerPids.Count -gt 0 -and $Alive.Count -ne 0) { Fail "cleanup: server processes still running: $($Alive -join ', ')" }
    elseif ($ServerPids.Count -gt 0 -and $KillCode -ne 0) { Fail "cleanup: kill-server exited $KillCode ($Kill)" }
    else { Pass "cleanup: namespace $Ns has no server left" }
    $DefaultAfter = Get-DefaultLs
    if ($DefaultAfter -ceq $DefaultBefore) { Pass "default namespace ls unchanged" }
    else { Fail "default namespace ls changed:`n$DefaultBefore`n---`n$DefaultAfter" }
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0 -or -not $SetupOk) { exit 1 }
exit 0
