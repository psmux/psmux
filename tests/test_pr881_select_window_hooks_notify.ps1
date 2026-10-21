# select-window hooks, control notifications and the last window (PR #881).
#
# Measured on tmux 3.8 (WSL, -f /dev/null), two windows of two panes, window 0
# current and window 1 last:
# - `select-window -t X:1.1` runs after-select-window and a control client
#   sees %session-window-changed and no %window-pane-changed: no pane moves.
# - A target that names nothing (`X:{end}.9`, `X:1.9`, `-n -t X:{end}.9`) is
#   %error with "can't find pane: 9", no notification and no hook. tmux has no
#   before-select-window hook; psmux has one, and it must not run for a
#   target that the command then refuses, since tmux resolves a target before
#   the command runs at all.
# - Run from a hook (the server's own command path), select-window of the
#   CURRENT window (`:0`, `:0.1`, `:.1`) changes nothing, window 1 stays the
#   last window (session.c:505, session_set_current returns early), and
#   `:1` swaps current and last.
#
# psmux ran before-select-window first and checked the target after, so a
# refused target still ran it; and the in-process path made the current
# window its own last window.
#
# Isolation: a unique -L namespace per run, a PSMUX_DATA_DIR, USERPROFILE and
# HOME of its own, a one line config, cleanup only with `-L <ns> kill-server`.
# PSMUX_TEST_EXE picks the binary (default: psmux on PATH).

$ErrorActionPreference = "Stop"
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($Ok, $m, $Detail) { if ($Ok) { Pass $m } else { Fail "$m -> $Detail" } }

$Psmux = $env:PSMUX_TEST_EXE
if (-not $Psmux) { $Found = Get-Command psmux -ErrorAction SilentlyContinue; if ($Found) { $Psmux = $Found.Source } }
if (-not $Psmux) { Write-Host "psmux binary not found"; exit 2 }
$Psmux = (Resolve-Path $Psmux).Path

$RootBase = if ($env:PSMUX_TEST_ROOT) { $env:PSMUX_TEST_ROOT } else { [System.IO.Path]::GetTempPath() }
$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$Root = Join-Path $RootBase ("psmux_swh_" + $Tag)

$TouchedNames = @('PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'USERPROFILE', 'HOME', 'TMUX', 'TMUX_PANE',
    'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL',
    'PSMUX_SERVER_MODE', 'PSMUX_REMOTE_ATTACH')
$SavedEnv = @{}
foreach ($n in $TouchedNames) { $SavedEnv[$n] = [Environment]::GetEnvironmentVariable($n) }
function Restore-Env {
    foreach ($n in $TouchedNames) {
        if ($null -eq $SavedEnv[$n]) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue } else { Set-Item "Env:$n" $SavedEnv[$n] }
    }
}

function Invoke-Psmux([string[]]$CliArgs, [int]$TimeoutMs = 15000) {
    $Info = New-Object System.Diagnostics.ProcessStartInfo
    $Info.FileName = $Psmux
    foreach ($a in @("-L", $script:Ns) + $CliArgs) { $Info.ArgumentList.Add($a) }
    $Info.UseShellExecute = $false
    $Info.RedirectStandardOutput = $true
    $Info.RedirectStandardError = $true
    $Proc = [System.Diagnostics.Process]::Start($Info)
    $Out = $Proc.StandardOutput.ReadToEndAsync(); $Err = $Proc.StandardError.ReadToEndAsync()
    if (-not $Proc.WaitForExit($TimeoutMs)) { $Proc.Kill(); $Proc.WaitForExit(); throw "timed out: $($CliArgs -join ' ')" }
    $Proc.WaitForExit()
    return @{ Code = $Proc.ExitCode; Out = $Out.Result.Trim(); Err = $Err.Result.Trim() }
}
function Step([string[]]$CliArgs) {
    $r = Invoke-Psmux $CliArgs
    if ($r.Code -ne 0) { throw "step '$($CliArgs -join ' ')' failed: exit $($r.Code), $($r.Err)" }
    return $r
}
function Get-Windows { (Step @("list-windows", "-t", "X", "-F", "#{window_index}:#{window_active}:#{window_last_flag}")).Out -split "`r?`n" -join " " }
function Get-Panes { (Step @("list-panes", "-s", "-t", "X", "-F", "#{window_index}.#{pane_index}:#{pane_active}")).Out -split "`r?`n" -join " " }
function Get-Hooks { "b=" + (Step @("show-options", "-gv", "@swh_b")).Out + " a=" + (Step @("show-options", "-gv", "@swh_a")).Out }

# Window 0 current, window 1 last, the first pane active in both, hook
# markers cleared.
function Reset-State {
    Step @("select-window", "-t", "X:1") | Out-Null
    Step @("select-pane", "-t", "X:1.$($script:P1)") | Out-Null
    Step @("select-window", "-t", "X:0") | Out-Null
    Step @("select-pane", "-t", "X:0.$($script:P1)") | Out-Null
    Step @("set-option", "-g", "@swh_b", "") | Out-Null
    Step @("set-option", "-g", "@swh_a", "") | Out-Null
}

function Start-Control {
    $Info = New-Object System.Diagnostics.ProcessStartInfo
    $Info.FileName = $Psmux
    foreach ($a in @("-L", $script:Ns, "-C", "attach-session", "-t", "X")) { $Info.ArgumentList.Add($a) }
    $Info.UseShellExecute = $false
    $Info.RedirectStandardInput = $true; $Info.RedirectStandardOutput = $true; $Info.RedirectStandardError = $true
    $script:Cc = [System.Diagnostics.Process]::Start($Info)
    $script:CcErr = $script:Cc.StandardError.ReadToEndAsync()
    $script:CcLine = $null
}
# Every line the control client printed until it has been quiet for $QuietMs.
function Read-Control([int]$QuietMs = 700) {
    $Lines = @()
    while ($true) {
        if ($null -eq $script:CcLine) { $script:CcLine = $script:Cc.StandardOutput.ReadLineAsync() }
        if (-not $script:CcLine.Wait($QuietMs)) { break }
        $l = $script:CcLine.Result; $script:CcLine = $null
        if ($null -eq $l) { break }
        $Lines += $l
    }
    return $Lines
}
function Stop-Control {
    if ($null -eq $script:Cc) { return }
    $p = $script:Cc; $script:Cc = $null
    try { $p.StandardInput.Close() } catch {}
    if (-not $p.WaitForExit(5000)) { Fail "cleanup: control client did not exit"; try { $p.Kill() } catch {} }
}

# One control command after a reset: the notifications it caused, the block
# kind, the hook markers and the windows.
function Test-Control([string]$Label, [string]$Command, [bool]$WantOk, [string]$WantWindows, [string]$WantHooks) {
    Reset-State
    $PanesBefore = Get-Panes
    Read-Control | Out-Null
    $script:Cc.StandardInput.WriteLine($Command); $script:Cc.StandardInput.Flush()
    Start-Sleep -Milliseconds 600
    $Lines = Read-Control
    $Kind = if ($Lines | Where-Object { $_ -match '^%error ' }) { "error" } elseif ($Lines | Where-Object { $_ -match '^%end ' }) { "end" } else { "none" }
    $Swc = @($Lines | Where-Object { $_ -match '^%session-window-changed ' }).Count
    $Wpc = @($Lines | Where-Object { $_ -match '^%window-pane-changed ' }).Count
    $Windows = Get-Windows; $Panes = Get-Panes; $Hooks = Get-Hooks
    $WantKind = if ($WantOk) { "end" } else { "error" }
    $WantSwc = if ($WantOk -and $WantWindows -ne $script:StartWindows) { 1 } else { 0 }
    $Ok = $Kind -eq $WantKind -and $Swc -eq $WantSwc -and $Wpc -eq 0 -and $Windows -ceq $WantWindows -and $Panes -ceq $PanesBefore -and $Hooks -ceq $WantHooks
    Check $Ok "$Label (control: $Command)" "block $Kind, session-window-changed x$Swc (want $WantSwc), window-pane-changed x$Wpc, windows '$Windows' (want '$WantWindows'), panes '$Panes' (was '$PanesBefore'), hooks '$Hooks' (want '$WantHooks'), lines: $($Lines -join ' / ')"
}

function Test-Cli([string]$Label, [string[]]$CliArgs, [int]$WantCode, [string]$WantWindows, [string]$WantHooks) {
    Reset-State
    $r = Invoke-Psmux $CliArgs
    Start-Sleep -Milliseconds 400
    $Windows = Get-Windows; $Hooks = Get-Hooks
    Check ($r.Code -eq $WantCode -and $Windows -ceq $WantWindows -and $Hooks -ceq $WantHooks) "$Label ($($CliArgs -join ' '))" "exit $($r.Code) '$($r.Err)', windows '$Windows' (want '$WantWindows'), hooks '$Hooks' (want '$WantHooks')"
}

# A hook runs select-window inside the server: after-new-window fires it and
# `new-window -d` leaves windows 0 and 1 alone apart from what it did.
function Test-InProcess([string]$Label, [string]$HookCommand, [string]$WantWindows) {
    Reset-State
    Step @("set-hook", "-g", "after-new-window", $HookCommand) | Out-Null
    try {
        Step @("new-window", "-d", "-t", "X:5") | Out-Null
        Start-Sleep -Milliseconds 800
        $Windows = Get-Windows
    }
    finally {
        Step @("set-hook", "-gu", "after-new-window") | Out-Null
        Invoke-Psmux @("kill-window", "-t", "X:5") | Out-Null
    }
    Check ($Windows -ceq "$WantWindows 5:0:0") "$Label (hook: $HookCommand)" "windows '$Windows' (want '$WantWindows 5:0:0')"
}

$SetupOk = $true
foreach ($Base in @(0, 1)) {
    $script:Ns = "swh" + $Tag + "b" + $Base
    $DataDir = Join-Path $Root "data$Base"
    $HomeDir = Join-Path $Root "home$Base"
    $ConfigFile = Join-Path $Root "base$Base.conf"
    $script:P1 = $Base; $P2 = $Base + 1
    $ServerPids = @()
    $RunOk = $false
    try {
        New-Item -ItemType Directory -Force $DataDir, $HomeDir | Out-Null
        [System.IO.File]::WriteAllText($ConfigFile, "set -g pane-base-index $Base`n")
        [System.IO.File]::WriteAllText((Join-Path $HomeDir ".psmux.conf"), "")
        [System.IO.File]::WriteAllText((Join-Path $HomeDir ".tmux.conf"), "")
        foreach ($n in $TouchedNames) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
        $env:PSMUX_DATA_DIR = $DataDir; $env:PSMUX_CONFIG_FILE = $ConfigFile
        $env:USERPROFILE = $HomeDir; $env:HOME = $HomeDir

        Write-Host "`n=== select-window hooks and notifications, pane-base-index $Base ===" -ForegroundColor Cyan
        Write-Host "  Binary: $Psmux  Namespace: $($script:Ns)"
        & $Psmux -L $script:Ns new-session -d -s X -x 120 -y 40 *> $null
        $Deadline = (Get-Date).AddSeconds(15)
        do { $Probe = Invoke-Psmux @("has-session", "-t", "X") 5000; if ($Probe.Code -eq 0) { break }; Start-Sleep -Milliseconds 200 } while ((Get-Date) -lt $Deadline)
        foreach ($f in Get-ChildItem $DataDir -Filter "*.pid" -ErrorAction SilentlyContinue) {
            $ServerPids += [int](([System.IO.File]::ReadAllText($f.FullName)).Trim().Split(":")[0])
        }
        if ($Probe.Code -ne 0) { throw "session X did not come up" }
        Step @("split-window", "-t", "X:0") | Out-Null
        Step @("new-window", "-t", "X:1") | Out-Null
        Step @("split-window", "-t", "X:1") | Out-Null
        Start-Sleep -Milliseconds 1200
        Step @("set-hook", "-g", "before-select-window", "set-option -g @swh_b fired") | Out-Null
        Step @("set-hook", "-g", "after-select-window", "set-option -g @swh_a fired") | Out-Null
        Reset-State
        $script:StartWindows = Get-Windows
        if ($script:StartWindows -cne "0:1:0 1:0:1") { throw "unexpected start windows '$($script:StartWindows)'" }
        $Moved = "0:0:1 1:1:0"
        $Fired = "b=fired a=fired"; $None = "b= a="

        Write-Host "`n--- CLI: hooks only for a target that resolves ---" -ForegroundColor Yellow
        Test-Cli "1 another window's pane: both hooks" @("select-window", "-t", "X:1.$P2") 0 $Moved $Fired
        Test-Cli "2 a missing pane under {end}: no hook" @("select-window", "-t", "X:{end}.9") 1 $script:StartWindows $None
        Test-Cli "3 -n with a missing pane: no hook" @("select-window", "-n", "-t", "X:{end}.9") 1 $script:StartWindows $None
        Test-Cli "4 -p with a missing pane: no hook" @("select-window", "-p", "-t", "X:{end}.9") 1 $script:StartWindows $None
        Test-Cli "5 a missing window: no hook" @("select-window", "-t", "X:{swhmissing}") 1 $script:StartWindows $None
        Test-Cli "6 -n with a valid pane part: both hooks" @("select-window", "-n", "-t", "X:{end}.$P2") 0 $Moved $Fired

        Write-Host "`n--- control client: notifications ---" -ForegroundColor Yellow
        Start-Control
        try {
            Test-Control "7 another window's pane: window changed, no pane changed" "select-window -t X:1.$P2" $true $Moved $Fired
            Test-Control "8 the current window's pane: no notification" "select-window -t X:0.$P2" $true $script:StartWindows $Fired
            Test-Control "9 a missing pane under {end}: %error, nothing else" "select-window -t X:{end}.9" $false $script:StartWindows $None
            Test-Control "10 a missing pane by number: %error, nothing else" "select-window -t X:1.9" $false $script:StartWindows $None
            Test-Control "11 -n with a missing pane: %error, nothing else" "select-window -n -t X:{end}.9" $false $script:StartWindows $None
        }
        finally { Stop-Control }

        Write-Host "`n--- the server's own command path keeps the last window ---" -ForegroundColor Yellow
        Step @("set-hook", "-gu", "before-select-window") | Out-Null
        Step @("set-hook", "-gu", "after-select-window") | Out-Null
        Test-InProcess "12 the current window: window 1 stays last" "select-window -t :0" $script:StartWindows
        Test-InProcess "13 the current window by pane: window 1 stays last" "select-window -t :0.$P2" $script:StartWindows
        Test-InProcess "14 the current window's pane alone: window 1 stays last" "select-window -t :.$P2" $script:StartWindows
        Test-InProcess "15 the other window: current and last swap" "select-window -t :1" $Moved
        $RunOk = $true
    }
    catch { Fail "setup or unexpected error: $_" }
    finally {
        Stop-Control
        $Kill = & $Psmux -L $script:Ns kill-server 2>&1
        $KillCode = $LASTEXITCODE
        $Deadline = (Get-Date).AddSeconds(10)
        do {
            $Alive = @($ServerPids | Where-Object { $p = Get-Process -Id $_ -ErrorAction SilentlyContinue; $p -and $p.Path -eq $Psmux })
            if ($Alive.Count -eq 0) { break }
            Start-Sleep -Milliseconds 200
        } while ((Get-Date) -lt $Deadline)
        Restore-Env
        if ($ServerPids.Count -gt 0 -and $Alive.Count -ne 0) { Fail "cleanup: server still running: $($Alive -join ', ')" }
        elseif ($ServerPids.Count -gt 0 -and $KillCode -ne 0) { Fail "cleanup: kill-server exited $KillCode ($Kill)" }
        else { Pass "cleanup: namespace $($script:Ns) has no server left" }
        if (-not $RunOk) { $SetupOk = $false }
    }
}
Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0 -or -not $SetupOk) { exit 1 }
exit 0
