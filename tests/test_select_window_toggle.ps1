# select-window -T (tmux 3.8, cmd-select-window.c:131-138): when the target is
# already the current window, select the last window instead; with no last
# window it is the error "no last window" and no after-select-window hook.
# A target that is not current is selected as usual.
#
# psmux ignored -T on every route: the CLI dropped the flag before sending,
# the server request had no field for it, and the server's own command path
# (hooks, key bindings, the command prompt) never looked at it, so
# `select-window -T -t <current>` changed nothing.
#
# Routes: CLI, control client, a hook (server command path), and with an
# attached client the command prompt and a key binding (WriteConsoleInput).
#
# Isolation: a unique -L namespace, a PSMUX_DATA_DIR, USERPROFILE and HOME of
# its own, cleanup only with `-L <ns> kill-server`. PSMUX_TEST_EXE picks the
# binary (default: psmux on PATH). Exit code is the fail count.

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

$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$Root = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_swt_" + $Tag)
$script:Ns = "swt" + $Tag

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
# "index:active:last" for every window of session $S.
function Get-Windows([string]$S = "X") { (Step @("list-windows", "-t", $S, "-F", "#{window_index}:#{window_active}:#{window_last_flag}")).Out -split "`r?`n" -join " " }
function Get-Hook { (Step @("show-options", "-gv", "@swt_a")).Out }

# Three windows; window 2 current, window 1 last; the hook marker cleared.
$Start = "0:0:0 1:0:1 2:1:0"
function Reset-State([string]$S = "X") {
    Step @("select-window", "-t", "${S}:0") | Out-Null
    Step @("select-window", "-t", "${S}:1") | Out-Null
    Step @("select-window", "-t", "${S}:2") | Out-Null
    if ($S -eq "X") { Step @("set-option", "-g", "@swt_a", "none") | Out-Null }
}

function Test-Cli([string]$Label, [string[]]$CliArgs, [int]$WantCode, [string]$WantWindows, [string]$WantHook, [string]$WantErr = "") {
    Reset-State
    $r = Invoke-Psmux $CliArgs
    Start-Sleep -Milliseconds 300
    $Windows = Get-Windows; $Hook = Get-Hook
    $ErrOk = (-not $WantErr) -or ($r.Err -match [regex]::Escape($WantErr))
    Check ($r.Code -eq $WantCode -and $Windows -ceq $WantWindows -and $Hook -ceq $WantHook -and $ErrOk) "$Label (psmux $($CliArgs -join ' '))" "exit $($r.Code) '$($r.Err)', windows '$Windows' (want '$WantWindows'), hook '$Hook' (want '$WantHook')"
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
function Test-Control([string]$Label, [string]$Command, [bool]$WantOk, [string]$WantWindows, [string]$WantHook) {
    Reset-State
    Read-Control | Out-Null
    $script:Cc.StandardInput.WriteLine($Command); $script:Cc.StandardInput.Flush()
    Start-Sleep -Milliseconds 600
    $Lines = Read-Control
    $Kind = if ($Lines | Where-Object { $_ -match '^%error ' }) { "error" } elseif ($Lines | Where-Object { $_ -match '^%end ' }) { "end" } else { "none" }
    $Windows = Get-Windows; $Hook = Get-Hook
    $WantKind = if ($WantOk) { "end" } else { "error" }
    Check ($Kind -eq $WantKind -and $Windows -ceq $WantWindows -and $Hook -ceq $WantHook) "$Label (control: $Command)" "block $Kind (want $WantKind), windows '$Windows' (want '$WantWindows'), hook '$Hook' (want '$WantHook'), lines: $($Lines -join ' / ')"
}

# A hook runs select-window inside the server: after-new-window fires it and
# `new-window -d -t X:5` leaves the current window where it was.
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

$ServerPids = @()
$Tui = $null
try {
    $DataDir = Join-Path $Root "data"; $HomeDir = Join-Path $Root "home"
    New-Item -ItemType Directory -Force $DataDir, $HomeDir | Out-Null
    $ConfigFile = Join-Path $Root "empty.conf"
    [System.IO.File]::WriteAllText($ConfigFile, "")
    [System.IO.File]::WriteAllText((Join-Path $HomeDir ".psmux.conf"), "")
    [System.IO.File]::WriteAllText((Join-Path $HomeDir ".tmux.conf"), "")
    foreach ($n in $TouchedNames) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
    $env:PSMUX_DATA_DIR = $DataDir; $env:PSMUX_CONFIG_FILE = $ConfigFile
    $env:USERPROFILE = $HomeDir; $env:HOME = $HomeDir

    Write-Host "`n=== select-window -T ===" -ForegroundColor Cyan
    Write-Host "  Binary: $Psmux  Namespace: $($script:Ns)"
    & $Psmux -L $script:Ns new-session -d -s X -x 120 -y 40 *> $null
    $Deadline = (Get-Date).AddSeconds(15)
    do { $Probe = Invoke-Psmux @("has-session", "-t", "X") 5000; if ($Probe.Code -eq 0) { break }; Start-Sleep -Milliseconds 200 } while ((Get-Date) -lt $Deadline)
    if ($Probe.Code -ne 0) { throw "session X did not come up" }
    Step @("new-window", "-t", "X:1") | Out-Null
    Step @("new-window", "-t", "X:2") | Out-Null
    Start-Sleep -Milliseconds 800
    Step @("set-hook", "-g", "after-select-window", "set-option -g @swt_a fired") | Out-Null
    Reset-State
    $Got = Get-Windows
    if ($Got -cne $Start) { throw "unexpected start windows '$Got'" }
    $ToLast = "0:0:0 1:1:0 2:0:1"
    $ToZero = "0:1:0 1:0:0 2:0:1"

    Write-Host "`n--- CLI ---" -ForegroundColor Yellow
    Test-Cli "1 -T on the current window selects the last window" @("select-window", "-T", "-t", "X:2") 0 $ToLast "fired"
    Test-Cli "2 -T on another window selects it" @("select-window", "-T", "-t", "X:0") 0 $ToZero "fired"
    Test-Cli "3 -T with no -t is the current window" @("select-window", "-T") 0 $ToLast "fired"
    $Id2 = (Step @("display-message", "-t", "X:2", "-p", "#{window_id}")).Out
    Test-Cli "4 -T on the current window by id" @("select-window", "-T", "-t", $Id2) 0 $ToLast "fired"
    Test-Cli "5 without -T the current window stays" @("select-window", "-t", "X:2") 0 $Start "fired"

    Write-Host "`n--- control client ---" -ForegroundColor Yellow
    Start-Control
    try {
        Test-Control "6 -T on the current window selects the last window" "select-window -T -t X:2" $true $ToLast "fired"
        Test-Control "7 -T on another window selects it" "select-window -T -t X:0" $true $ToZero "fired"
    }
    finally { Stop-Control }

    Write-Host "`n--- the server's own command path (a hook) ---" -ForegroundColor Yellow
    Step @("set-hook", "-gu", "after-select-window") | Out-Null
    Test-InProcess "8 -T on the current window selects the last window" "select-window -T -t :2" $ToLast
    Test-InProcess "9 -T with no -t selects the last window" "select-window -T" $ToLast
    Test-InProcess "10 -T on another window selects it" "select-window -T -t :0" $ToZero
    Test-InProcess "11 without -T the current window stays" "select-window -t :2" $Start

    Write-Host "`n--- no last window ---" -ForegroundColor Yellow
    & $Psmux -L $script:Ns new-session -d -s Y -x 80 -y 20 *> $null
    Start-Sleep -Seconds 2
    $r = Invoke-Psmux @("select-window", "-T", "-t", "Y:0")
    Check ($r.Code -eq 1 -and $r.Err -match "no last window") "12 -T with no last window is 'no last window', exit 1" "exit $($r.Code) '$($r.Err)'"
    $Wy = Get-Windows "Y"
    Check ($Wy -ceq "0:1:0") "13 and the window stays" "windows '$Wy'"

    Write-Host "`n--- attached client: command prompt and key binding ---" -ForegroundColor Yellow
    $Injector = Join-Path $Root "swt_injector.exe"
    $Csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $Csc /nologo /optimize /out:$Injector (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
    . (Join-Path $PSScriptRoot "injector_guard.ps1")
    $Tui = Start-Process -FilePath $Psmux -ArgumentList "-L", $script:Ns, "new-session", "-s", "Z" -PassThru
    $Deadline = (Get-Date).AddSeconds(15)
    do { $Probe = Invoke-Psmux @("has-session", "-t", "Z") 5000; if ($Probe.Code -eq 0) { break }; Start-Sleep -Milliseconds 200 } while ((Get-Date) -lt $Deadline)
    Start-Sleep -Seconds 2
    Step @("new-window", "-t", "Z:1") | Out-Null
    Step @("new-window", "-t", "Z:2") | Out-Null
    Step @("bind-key", "-t", "Z", "Y", "select-window", "-T") | Out-Null
    Start-Sleep -Milliseconds 500
    Reset-State "Z"
    if (Invoke-GuardedInjector -Injector $Injector -ClientPid $Tui.Id -Keys "^b{SLEEP:400}:{SLEEP:500}select-window -T{ENTER}" -RequireDelivery) {
        Start-Sleep -Milliseconds 1200
        $Wz = Get-Windows "Z"
        Check ($Wz -ceq $ToLast) "14 command prompt: select-window -T selects the last window" "windows '$Wz' (want '$ToLast')"
    } else { Write-Host "  [SKIP] 14 command prompt: $($script:InjectorBlocked)" -ForegroundColor DarkYellow }
    Reset-State "Z"
    if (Invoke-GuardedInjector -Injector $Injector -ClientPid $Tui.Id -Keys "^b{SLEEP:400}Y" -RequireDelivery) {
        Start-Sleep -Milliseconds 1200
        $Wz = Get-Windows "Z"
        Check ($Wz -ceq $ToLast) "15 key binding: prefix Y (select-window -T) selects the last window" "windows '$Wz' (want '$ToLast')"
    } else { Write-Host "  [SKIP] 15 key binding: $($script:InjectorBlocked)" -ForegroundColor DarkYellow }
}
catch { Fail "setup or unexpected error: $_" }
finally {
    Stop-Control
    & $Psmux -L $script:Ns kill-server *> $null
    if ($Tui) {
        if (-not $Tui.WaitForExit(5000)) { try { Stop-Process -Id $Tui.Id -Force } catch {} }
    }
    Restore-Env
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}
Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
exit $script:Failed
