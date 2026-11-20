# Issue #573: mouse stopped working after upgrading 3.3.6 -> 3.3.7
#
# Reported on Windows Server 2022 (build 20348) in Alacritty and WezTerm: no
# click-to-switch-pane, no wheel, while `show-options -g -v mouse` still reads
# `on`. Downgrading to 3.3.6 restored it.
#
# Root cause: c92aec0 (#457) gates send_mouse_enable() on build >= 22523.  That
# call is the only thing that writes the mouse-enable DECSET via a raw WriteFile
# on the console handle, deliberately bypassing ConPTY, and on builds whose
# ConPTY swallows the DECSET instead of forwarding it that bypass was the only
# way the terminal ever entered mouse reporting.  Server 2022 sits below the
# threshold, so 3.3.7 left it with no mouse-enable at all.  5415fd8 (#468) then
# routed local WezTerm through the same VT path.
#
# The gate is NOT being removed: on Win10 19045 an SGR mouse report fast-fails
# conhost (0xc0000409) and kills the pane process, which is strictly worse than
# a dead mouse.  237d93f added PSMUX_FORCE_MOUSE, so a user whose conhost does
# handle mouse can get it back without downgrading.
#
# Re measured on 3.4.0 (20aa1d7) and master (652d08b4) with build 20348 faked
# through PSMUX_FAKE_WIN_BUILD, against 3.3.6:
#
#     route                         3.3.6   3.4.0 / master   with PSMUX_FORCE_MOUSE=1
#     SSH (SSH_CONNECTION set)      ?1000h  absent           ?1000h
#     local WezTerm (TERM_PROGRAM)  absent  absent           ?1000h
#     local, no TERM_PROGRAM        absent  absent           absent (ConPTY's own ?1003;1006h only)
#
# so the default still differs from 3.3.6 only on the SSH route, and a real
# WezTerm or Alacritty window on this box clicks and scrolls with the build
# faked to 20348 (WezTerm ships its own OpenConsole.exe, so the inbox host's
# build is not the host it runs on).  Part 3 drives that when they exist.
#
# Part 1 and 2 host a real `psmux attach` inside a ConPTY and dump every
# outward byte. `\e[?1000h` appears ONLY in the bypass write, so its presence
# is an exact read of whether the gate fired.
#
# Safety: own -L namespace, honours PSMUX_DATA_DIR, binary from $env:PSMUX_BIN
# (else psmux on PATH). Nothing is killed by name; only the PIDs this script
# starts. The clipboard is saved and restored around the mouse layer.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i573m" + (Get-Random -Maximum 99999)
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:Started = @()
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow }
function Write-Info($msg) { Write-Host "  [info] $msg" -ForegroundColor DarkGray }

$env:PSMUX_RUN_NOPAUSE = '1'
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }
$probeVars = 'PSMUX_FAKE_WIN_BUILD','PSMUX_FORCE_MOUSE','TERM_PROGRAM','WEZTERM_PANE','SSH_TTY','SSH_CONNECTION','SSH_CLIENT','WT_SESSION'
$saved = @{}; foreach ($v in $probeVars) { $saved[$v] = [Environment]::GetEnvironmentVariable($v) }
function Clear-Probe { foreach ($v in $probeVars) { Remove-Item "env:$v" -EA SilentlyContinue } }

$work = Join-Path $env:TEMP "psmux_i573_$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) { $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe" }
$capExe = Join-Path $work "conptycap.exe"
& $csc /nologo /optimize /out:$capExe (Join-Path $PSScriptRoot "conptycap.cs") 2>&1 | Out-Null
if (-not (Test-Path $capExe)) { Write-Fail "could not compile the ConPTY capture harness"; exit 1 }

function P { & $PSMUX -L $NS @args 2>&1 }

# Host `psmux attach` under a real ConPTY with the given environment and say
# whether the bypass mouse-enable block reached the terminal.
function Measure-MouseEnable([string]$Tag, [string]$FakeBuild, [string]$ForceMouse, [string]$TermProgram, [switch]$Ssh) {
    Clear-Probe
    $env:PSMUX_NO_WARM = "1"
    if ($FakeBuild)   { $env:PSMUX_FAKE_WIN_BUILD = $FakeBuild }
    if ($ForceMouse)  { $env:PSMUX_FORCE_MOUSE = $ForceMouse }
    if ($TermProgram) { $env:TERM_PROGRAM = $TermProgram }
    if ($Ssh) { $env:SSH_CONNECTION = '10.0.0.5 50000 10.0.0.6 22'; $env:SSH_CLIENT = '10.0.0.5 50000 22'; $env:SSH_TTY = 'windows-pty' }
    $S = "m$Tag"
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P set-option -g mouse on | Out-Null
    $mouseOpt = ((P show-options -g -v mouse) -join "").Trim()
    $out = Join-Path $work "$Tag.bin"
    $env:CONPTYCAP_DRAIN_MS = "7000"
    $p = Start-Process -FilePath $capExe -ArgumentList @($out, "120", "30", "0", "`"$PSMUX`"", "-L", $NS, "attach", "-t", $S) -PassThru -WindowStyle Hidden
    $script:Started += $p.Id
    $null = $p.WaitForExit(30000)
    if (-not $p.HasExited) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
    $bytes = if (Test-Path $out) { [IO.File]::ReadAllBytes($out) } else { @() }
    $text = -join ($bytes | ForEach-Object { [char]$_ })
    P kill-session -t $S | Out-Null
    Clear-Probe
    Remove-Item Env:\PSMUX_NO_WARM -EA SilentlyContinue
    [pscustomobject]@{ Tag = $Tag; Bytes = $bytes.Count; MouseOpt = $mouseOpt; HasEnable = $text.Contains("$([char]27)[?1000h") }
}

Write-Host "`n=== Issue #573: the ConPTY mouse gate and its escape hatch ===" -ForegroundColor Cyan
Write-Host "psmux: $PSMUX ($((& $PSMUX -V 2>&1 | Out-String).Trim() -replace '\s+',' '))  namespace: $NS" -ForegroundColor DarkGray

Write-Host "`n[Part 1] the WezTerm VT path" -ForegroundColor Yellow
$r1 = Measure-MouseEnable -Tag "gated" -FakeBuild "20348" -TermProgram "WezTerm"
Write-Info "$($r1.Tag): bytes=$($r1.Bytes) mouse='$($r1.MouseOpt)' enableBlock=$($r1.HasEnable)"
if ($r1.MouseOpt -eq "on") { Write-Pass "the mouse option itself reads 'on' (matches the reporter's screenshot)" }
else { Write-Fail "setup: mouse option is '$($r1.MouseOpt)', expected 'on'" }
if ($r1.Bytes -gt 0 -and -not $r1.HasEnable) { Write-Pass "no mouse-enable bypass on gated build 20348 by default (#457 safety intact)" }
else { Write-Fail "build 20348 default: bytes=$($r1.Bytes) enableBlock=$($r1.HasEnable)" }
$r2 = Measure-MouseEnable -Tag "forced" -FakeBuild "20348" -ForceMouse "1" -TermProgram "WezTerm"
Write-Info "$($r2.Tag): bytes=$($r2.Bytes) enableBlock=$($r2.HasEnable)"
# The client's bytes are read back through an inbox pseudo console, and the
# one of Windows 10 (19045) passes no mouse-enable on at all (it has no ConPTY
# mouse, #457), so where one is expected it cannot be seen there. These checks
# fake the build psmux sees anyway (20348, 26200) and pass on Windows 11: what
# differs on Windows 10 is the host reading the bytes back.
$NoMouseOut = [Environment]::OSVersion.Version.Build -lt 22000
if ($r2.HasEnable) { Write-Pass "PSMUX_FORCE_MOUSE=1 restores the mouse-enable on build 20348" }
elseif ($NoMouseOut) { Write-Host "  [SKIP] Windows 10 inbox pseudo console passes no mouse-enable on, its presence cannot be read there" -ForegroundColor Yellow }
else { Write-Fail "PSMUX_FORCE_MOUSE=1 did not restore the mouse-enable" }
$r3 = Measure-MouseEnable -Tag "modern" -FakeBuild "26200" -TermProgram "WezTerm"
Write-Info "$($r3.Tag): bytes=$($r3.Bytes) enableBlock=$($r3.HasEnable)"
if ($r3.HasEnable) { Write-Pass "a modern build still emits the mouse-enable with no override" }
elseif ($NoMouseOut) { Write-Host "  [SKIP] Windows 10 inbox pseudo console passes no mouse-enable on, its presence cannot be read there" -ForegroundColor Yellow }
else { Write-Fail "REGRESSION: a modern build stopped emitting the mouse-enable" }
$r4 = Measure-MouseEnable -Tag "pinnedoff" -FakeBuild "26200" -ForceMouse "0" -TermProgram "WezTerm"
Write-Info "$($r4.Tag): bytes=$($r4.Bytes) enableBlock=$($r4.HasEnable)"
if ($r4.Bytes -gt 0 -and -not $r4.HasEnable) { Write-Pass "PSMUX_FORCE_MOUSE=0 pins mouse off on a modern build" }
else { Write-Fail "PSMUX_FORCE_MOUSE=0: bytes=$($r4.Bytes) enableBlock=$($r4.HasEnable)" }

Write-Host "`n[Part 2] the SSH path, the one 3.3.6 served on build 20348" -ForegroundColor Yellow
$s1 = Measure-MouseEnable -Tag "sshgated" -FakeBuild "20348" -Ssh
$s2 = Measure-MouseEnable -Tag "sshforced" -FakeBuild "20348" -ForceMouse "1" -Ssh
$s3 = Measure-MouseEnable -Tag "sshmodern" -FakeBuild "26200" -Ssh
Write-Info "ssh 20348=$($s1.HasEnable) ssh 20348 forced=$($s2.HasEnable) ssh 26200=$($s3.HasEnable)"
if ($s1.Bytes -gt 0 -and -not $s1.HasEnable) { Write-Pass "SSH on 20348: gated by default" } else { Write-Fail "SSH on 20348 default: bytes=$($s1.Bytes) enableBlock=$($s1.HasEnable)" }
if ($s2.HasEnable) { Write-Pass "SSH on 20348 with PSMUX_FORCE_MOUSE=1: mouse-enable emitted" } elseif ($NoMouseOut) { Write-Host "  [SKIP] Windows 10 inbox pseudo console passes no mouse-enable on, its presence cannot be read there" -ForegroundColor Yellow } else { Write-Fail "SSH on 20348 forced: no mouse-enable" }
if ($s3.HasEnable) { Write-Pass "SSH on a modern build: mouse-enable emitted" } elseif ($NoMouseOut) { Write-Host "  [SKIP] Windows 10 inbox pseudo console passes no mouse-enable on, its presence cannot be read there" -ForegroundColor Yellow } else { Write-Fail "SSH on 26200: no mouse-enable" }

Write-Host "`n[Part 3] a real wheel in a real WezTerm window, build faked to 20348" -ForegroundColor Yellow
$wez = "C:\Program Files\WezTerm\wezterm-gui.exe"
$wheelSrc = Join-Path $PSScriptRoot "issue657_gui_wheel.cs"
$wheelExe = Join-Path $work "gui_wheel.exe"
if (Test-Path $wheelSrc) { & $csc /nologo /out:$wheelExe $wheelSrc 2>&1 | Out-Null }
if (-not (Test-Path $wez) -or -not (Test-Path $wheelExe)) {
    Write-Skip "WezTerm or the window wheel harness is not available"
} else {
    $clip = try { Get-Clipboard -Raw } catch { $null }
    $launch = Join-Path $work "wez.cmd"
@"
@echo off
set PSMUX_DATA_DIR=$($env:PSMUX_DATA_DIR)
set PSMUX_NO_WARM=1
set PSMUX_FAKE_WIN_BUILD=20348
"$PSMUX" -L $NS -f NUL new-session -s wz
"@ | Set-Content -Path $launch -Encoding ASCII
    $w = Start-Process -FilePath $wez -ArgumentList '-n','--config','initial_cols=120','--config','initial_rows=30','start','--always-new-process','--','cmd.exe','/c',$launch -PassThru
    $script:Started += $w.Id
    for ($i = 0; $i -lt 60; $i++) { P has-session -t wz | Out-Null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 500 }
    Start-Sleep -Seconds 4
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "psmux did not come up inside WezTerm"
    } else {
        P set -g mouse on | Out-Null
        1..60 | ForEach-Object { P send-keys -t wz "echo r$_" Enter | Out-Null }
        Start-Sleep -Seconds 2
        $before = ((P display-message -p -t wz '#{pane_in_mode}') -join '').Trim()
        $o = & $wheelExe $w.Id up 3 300 200 2>&1
        Write-Info ("$o" -replace "`r?`n", " ")
        Start-Sleep -Milliseconds 1500
        $after = ((P display-message -p -t wz '#{pane_in_mode}') -join '').Trim()
        $pos = ((P display-message -p -t wz '#{scroll_position}') -join '').Trim()
        Write-Info "pane_in_mode $before -> $after, scroll_position=$pos"
        if ($before -ne '1' -and $after -eq '1' -and [int]("0$pos" -replace '\D','') -gt 0) {
            Write-Pass "a real WezTerm wheel scrolls psmux with the build faked to 20348"
        } else {
            Write-Fail "WezTerm wheel on faked 20348: in_mode $before -> $after, scroll_position=$pos"
        }
    }
    try { Stop-Process -Id $w.Id -Force -EA SilentlyContinue } catch {}
    if ($null -ne $clip) { try { Set-Clipboard -Value $clip } catch {} }
}

P kill-server | Out-Null
foreach ($id in $script:Started) { try { Stop-Process -Id $id -Force -EA Stop } catch {} }
foreach ($v in $probeVars) { if ($null -ne $saved[$v]) { [Environment]::SetEnvironmentVariable($v, $saved[$v]) } }
Remove-Item -Recurse -Force $work -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
