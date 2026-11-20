# Issue #597: "SGR mouse events not forwarded to pane apps" on Windows 10.
# Claude Code inside a psmux pane showed "Scroll wheel is sending arrow keys,
# use PgUp/PgDn to scroll" and the wheel did not scroll it.
#
# What the platform actually does (measured, see PART 1):
#   * Under ConPTY a client's OWN mouse DECSET bytes NEVER reach the terminal.
#     Writing \e[?1000h\e[?1002h\e[?1003h\e[?1006h to stdout, by WriteFile on the
#     raw console handle exactly as much as by WriteConsoleW, puts ZERO
#     private-mode bytes on the pty output pipe. conhost absorbs them.
#   * SetConsoleMode(hIn, ENABLE_MOUSE_INPUT) makes conhost emit
#     \e[?1003;1006h to the terminal, and clearing the flag emits \e[?1003;1006l.
#
# So the Win32 console flag is the ONLY mouse registration channel a local
# client has. Windows Terminal drops a long lived local client's registration on
# its own (documented at src/client.rs, the periodic mouse-enable refresh), and
# the ONLY code that restores it is the ENABLE_MOUSE_INPUT re-assert inside
# ssh_input::send_mouse_keepalive. That re-assert used to sit behind the issue
# #457 build gate, so on every Windows build below CONPTY_MOUSE_MIN_BUILD (22523)
# the loss was permanent: the terminal, having been told \e[?1003;1006l, falls
# back to alternate-scroll and turns the wheel into Up/Down arrow keys, which
# psmux forwards straight into the pane.
#
# tmux parity: tmux re-derives the outward mouse registration from the pane
# state on every reset and writes the DECSET itself (tty.c tty_update_mode,
# called from server-client.c server_client_reset_state), so it can never end up
# permanently unregistered. tmux also NEVER converts a wheel notch into arrow
# keys (that translation was removed); the arrows the reporter saw come from the
# outer terminal's alternate-scroll, which only engages because the terminal was
# told mouse reporting is off.
#
# Layers: platform invariant (real ConPTY byte capture), E2E console-mode oracle
#         with the build gate faked to the reporter's build, opt-out control,
#         byte-level wheel forwarding, Win32 attached TUI verification.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$SESSION = "test_i597"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$modeProbe = "$env:TEMP\psmux_i597_mouse_mode_probe.exe"
$regProbe  = "$env:TEMP\psmux_i597_mouse_registration_probe.exe"
$ptyCap    = "$env:TEMP\psmux_i597_conptycap.exe"
$inj       = "$env:TEMP\psmux_i597_mouse_injector.exe"
$child     = "$env:TEMP\psmux_i597_mouse_echo_child.exe"
$conread   = "$env:TEMP\psmux_i597_conread.exe"
$childLog  = "$env:TEMP\psmux_mouse_echo.txt"

foreach ($pair in @(
    @($modeProbe, "mouse_mode_probe.cs"),
    @($regProbe,  "mouse_registration_probe.cs"),
    @($ptyCap,    "conptycap.cs"),
    @($inj,       "mouse_injector.cs"),
    @($child,     "mouse_echo_child.cs"),
    @($conread,   "conread.cs"))) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

# psmux refuses to start an attached client from inside a pane, and the test
# runner itself may be running in one.
foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    [Environment]::SetEnvironmentVariable($v, $null)
}
$env:PSMUX_NO_WARM = "1"

function Read-Shared([string]$p) {
    $fs = [IO.File]::Open($p, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    $b = New-Object byte[] $fs.Length
    [void]$fs.Read($b, 0, $b.Length)
    $fs.Close()
    return $b
}
function Decode([byte[]]$b) {
    return -join ($b | ForEach-Object {
        if ($_ -eq 27) { '<ESC>' } elseif ($_ -ge 32 -and $_ -lt 127) { [char]$_ } else { '.' } })
}
function Cleanup([string]$s) {
    & $PSMUX kill-session -t $s 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Issue #597: the mouse keep-alive must re-register mouse on every build ===" -ForegroundColor Cyan

# ─────────────────────────────────────────────────────────────────────
# PART 1: platform invariant — which mechanism registers mouse at all
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Part 1] Under a real ConPTY, only SetConsoleMode registers mouse" -ForegroundColor Yellow
foreach ($case in @(
    @{ Mode = 'setconsolemode';   Expect = $true;  Why = 'SetConsoleMode(ENABLE_MOUSE_INPUT) must reach the terminal' },
    @{ Mode = 'decset_writefile'; Expect = $false; Why = 'raw WriteFile DECSET is absorbed by conhost' },
    @{ Mode = 'decset_writecon';  Expect = $false; Why = 'WriteConsoleW DECSET is absorbed by conhost' })) {
    $out = "$env:TEMP\psmux_i597_reg_$($case.Mode).bin"
    Remove-Item $out -Force -EA SilentlyContinue
    $p = Start-Process -FilePath $ptyCap -ArgumentList $out,"120","30","0",$regProbe,$case.Mode -PassThru
    Start-Sleep -Seconds 6
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 400
    if (-not (Test-Path $out)) { Write-Fail "no pty capture for $($case.Mode)"; continue }
    $txt = Decode (Read-Shared $out)
    $got = [bool]($txt -match '<ESC>\[\?[0-9;]*100[0236][0-9;]*h')
    if ($got -eq $case.Expect) { Write-Pass "$($case.Mode) -> registered=$got ($($case.Why))" }
    elseif ($case.Expect -and [Environment]::OSVersion.Version.Build -lt 22000) {
        # No ConPTY mouse before build 22523 (#457): the inbox pseudo console
        # of Windows 10 (19045) emits no mouse DECSET for ENABLE_MOUSE_INPUT
        # either, so nothing registers mouse there; a platform fact, no psmux.
        Write-Host "  [SKIP] $($case.Mode) -> registered=${got}: the Windows 10 pseudo console has no mouse to register" -ForegroundColor Yellow
    }
    else { Write-Fail "$($case.Mode) -> registered=$got, expected $($case.Expect) ($($case.Why))" }
}

# ─────────────────────────────────────────────────────────────────────
# PART 2: the regression itself — a mid-session registration loss must heal
# ─────────────────────────────────────────────────────────────────────
# The client's keep-alive fires every 30 s, so each case needs a >30 s window.
function Test-Keepalive([string]$Tag, [string]$FakeBuild, [string]$ForceMouse, [bool]$ExpectRecovery) {
    $s = "${SESSION}_$Tag"
    Cleanup $s
    [Environment]::SetEnvironmentVariable('PSMUX_FAKE_WIN_BUILD', $null)
    [Environment]::SetEnvironmentVariable('PSMUX_FORCE_MOUSE', $null)
    if ($FakeBuild)  { $env:PSMUX_FAKE_WIN_BUILD = $FakeBuild }
    if ($ForceMouse) { $env:PSMUX_FORCE_MOUSE = $ForceMouse }

    $proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$s -PassThru
    for ($i = 0; $i -lt 25; $i++) {
        Start-Sleep -Seconds 1
        & $PSMUX has-session -t $s 2>$null
        if ($LASTEXITCODE -eq 0) { break }
    }
    Start-Sleep -Seconds 2
    $start = & $modeProbe $proc.Id query 2>&1
    if ($start -notmatch 'mouse=True') {
        Write-Fail "$Tag : client console did not start with ENABLE_MOUSE_INPUT ($start)"
        Cleanup $s; try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
        return
    }
    $cleared = & $modeProbe $proc.Id clear 2>&1
    if ($cleared -notmatch 'after=0x[0-9A-F]{4} mouse=False') {
        Write-Fail "$Tag : could not clear ENABLE_MOUSE_INPUT ($cleared)"
        Cleanup $s; try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
        return
    }
    $recovered = $false
    for ($k = 0; $k -lt 10; $k++) {
        & $PSMUX send-keys -t $s "echo i597_$k" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 5
        if ((& $modeProbe $proc.Id query 2>&1) -match 'after=0x[0-9A-F]{4} mouse=True') { $recovered = $true; break }
    }
    if ($recovered -eq $ExpectRecovery) {
        Write-Pass "$Tag (build='$FakeBuild' force='$ForceMouse') -> recovered=$recovered as expected"
    } else {
        Write-Fail "$Tag (build='$FakeBuild' force='$ForceMouse') -> recovered=$recovered, expected $ExpectRecovery"
    }
    Cleanup $s
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    [Environment]::SetEnvironmentVariable('PSMUX_FAKE_WIN_BUILD', $null)
    [Environment]::SetEnvironmentVariable('PSMUX_FORCE_MOUSE', $null)
}

Write-Host "`n[Part 2] A cleared ENABLE_MOUSE_INPUT must be re-asserted by the keep-alive" -ForegroundColor Yellow
# The reporter's platform. This is the case that failed before the fix.
Test-Keepalive -Tag "win10"  -FakeBuild "19045" -ForceMouse ""  -ExpectRecovery $true
# Windows Server 2022, the other host class the #457 gate silently disarmed (#573).
Test-Keepalive -Tag "srv22"  -FakeBuild "20348" -ForceMouse ""  -ExpectRecovery $true
# Above the gate threshold: behaviour must be unchanged.
Test-Keepalive -Tag "modern" -FakeBuild "22631" -ForceMouse ""  -ExpectRecovery $true
# The explicit opt-out must still pin mouse dead.
Test-Keepalive -Tag "forced_off" -FakeBuild "19045" -ForceMouse "0" -ExpectRecovery $false

# ─────────────────────────────────────────────────────────────────────
# PART 3: byte level — a wheel notch reaches a mouse-aware pane app as SGR
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Part 3] The wheel still reaches a mouse-aware pane app as SGR" -ForegroundColor Yellow
$s3 = "${SESSION}_sgr"
Cleanup $s3
Remove-Item $childLog -Force -EA SilentlyContinue
$proc3 = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$s3 -PassThru
for ($i = 0; $i -lt 25; $i++) { Start-Sleep -Seconds 1; & $PSMUX has-session -t $s3 2>$null; if ($LASTEXITCODE -eq 0) { break } }
& $PSMUX set-option -t $s3 -g mouse on 2>&1 | Out-Null
$pane3 = ((& $PSMUX list-panes -t $s3 -F '#{pane_id}') | Select-Object -First 1).Trim()
& $PSMUX send-keys -t $pane3 ($child -replace '\\', '/') Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (((& $PSMUX capture-pane -t $pane3 -p 2>&1) | Out-String) -match 'MOUSE_ECHO_READY') {
    Write-Pass "mouse-reporting child is running in pane $pane3"
    $before = if (Test-Path $childLog) { (Get-Content $childLog).Count } else { 0 }
    & $inj $proc3.Id "up" 3 20 8 | Out-Null
    Start-Sleep -Milliseconds 1200
    & $inj $proc3.Id "down" 3 20 8 | Out-Null
    Start-Sleep -Milliseconds 1200
    $all = if (Test-Path $childLog) { Get-Content $childLog } else { @() }
    $new = if ($all.Count -gt $before) { $all[$before..($all.Count - 1)] } else { @() }
    $ups   = @($new | Where-Object { $_ -match '<ESC>\[<64;\d+;\d+M' }).Count
    $downs = @($new | Where-Object { $_ -match '<ESC>\[<65;\d+;\d+M' }).Count
    if ($ups -ge 3)   { Write-Pass "wheel up delivered $ups SGR reports (ESC[<64;col;rowM)" }   else { Write-Fail "wheel up delivered $ups SGR reports, expected 3" }
    if ($downs -ge 3) { Write-Pass "wheel down delivered $downs SGR reports (ESC[<65;col;rowM)" } else { Write-Fail "wheel down delivered $downs SGR reports, expected 3" }
} else {
    Write-Fail "mouse-reporting child did not start"
}

# ─────────────────────────────────────────────────────────────────────
# PART 4 (mandatory): Win32 attached TUI verification on a real console
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Part 4] Win32 attached TUI: the real client screen" -ForegroundColor Yellow
$screen = (& $conread $proc3.Id 40 2>&1) | Out-String
# psmux truncates the session name in the status bar, so match its visible stem.
$stem = $s3.Substring(0, [Math]::Min(8, $s3.Length))
if ($screen -match [regex]::Escape("[$stem")) {
    Write-Pass "attached client window renders the status bar for [$stem"
} else {
    Write-Fail "attached client window did not render the [$stem status bar"
}
if ($screen -match 'MOUSE_ECHO_READY') {
    Write-Pass "the mouse-aware child is visible on the real client screen"
} else {
    Write-Fail "the mouse-aware child is not visible on the real client screen"
}
$modeNow = & $modeProbe $proc3.Id query 2>&1
if ($modeNow -match 'mouse=True') {
    Write-Pass "attached client console still holds ENABLE_MOUSE_INPUT ($modeNow)"
} else {
    Write-Fail "attached client console lost ENABLE_MOUSE_INPUT ($modeNow)"
}

Cleanup $s3
try { Stop-Process -Id $proc3.Id -Force -EA SilentlyContinue } catch {}

Write-Host "`n=== Issue #597 results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
