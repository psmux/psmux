# Issue #735, byte level: what an attached client writes to its outer terminal
# about the cursor, at attach, in its frames and at detach.
#
# tmux sends no cursor shape at start (tty.c tty_start_tty) and sends the
# reset only to undo a shape it set itself (tty.c tty_update_cursor and
# tty_stop_tty, both guarded by `tty->cstyle != SCREEN_CURSOR_DEFAULT`), and
# its `cursor-style` defaults to `default` (options-table.c, .default_num = 0).
#
# tests/test_issue735_cursor_style_default.ps1 checks the option values; this
# one hosts a real attached client inside a CreatePseudoConsole in passthrough
# mode (tests/conptycap.cs, same approach as test_issue626) and greps the bytes.
#
# Before the fix (master dd695eaf): with no config the client wrote
#   ESC[?12h ESC[5 q (attach)  ESC[5 q (first frame)  ESC[0 q ESC[?12l (detach)
# and `cursor-style blinking-bar` resolved to ESC[0 q, no shape at all.

$ErrorActionPreference = "Continue"
$ESC  = [char]27
$SOCK = "i735b_$PID"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = $env:PSMUX_TEST_BIN }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }
Write-Host "binary: $PSMUX"

$work = Join-Path $env:TEMP "psmux_i735b_$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
$capExe = Join-Path $work "conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc) -and -not (Test-Path $capExe)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}
if (-not (Test-Path $capExe)) {
    Write-Skip "csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
    exit 0
}
$build = [Environment]::OSVersion.Version.Build
if ($build -lt 22621) {
    Write-Skip "ConPTY passthrough needs build 22621 or later (this is $build)"
    exit 0
}

# Start a session from a config of its own, attach a client inside the
# pseudoconsole, optionally have the pane print a DECSCUSR, detach from outside
# so the client runs its normal teardown, and return every byte it wrote.
function Capture-Case {
    param([string]$Name, [string[]]$Conf, [string]$PaneCode = "")
    $cf = Join-Path $work "$Name.conf"
    ((@("# written by test_issue735_cursor_attach_bytes") + $Conf) -join "`r`n") | Set-Content -Path $cf -Encoding ASCII
    & $PSMUX -L $SOCK -f $cf new-session -d -s $Name -x 100 -y 30 2>&1 | Out-Null
    $up = $false
    for ($i = 0; $i -lt 40; $i++) {
        & $PSMUX -L $SOCK has-session -t $Name 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { $up = $true; break }
        Start-Sleep -Milliseconds 250
    }
    if (-not $up) { return $null }
    $launch = Join-Path $work "attach_$Name.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
"$PSMUX" -L $SOCK attach -t $Name
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = Join-Path $work "client_$Name.bin"
    Remove-Item $outBin -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "20000"
    $cap = Start-Process -FilePath $capExe -ArgumentList @($outBin, "100", "30", "8", $launch) -WindowStyle Minimized -PassThru
    # Wait for the client to be attached before detaching it. A fixed sleep let
    # a slow (loaded) client be detached before it registered, so in sweep
    # 2026-11-21_01-34-45 the detach reset never got written (`held []`).
    $attached = $false
    for ($i = 0; $i -lt 100; $i++) {
        $cl = & $PSMUX -L $SOCK list-clients -t $Name 2>$null
        if ($cl) { $attached = $true; break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $attached) { Write-Host "  (diag) client for $Name never showed in list-clients after 10 s" }
    Start-Sleep -Milliseconds 1500
    if ($PaneCode) {
        # Print a marker after the DECSCUSR and wait for it: under load the pane
        # shell can take longer than a fixed 2 s to start and run the line.
        & $PSMUX -L $SOCK send-keys -t $Name ('Write-Host -NoNewline ([char]27 + "[' + $PaneCode + ' q"); Write-Host ("P735" + "DONE")') Enter 2>&1 | Out-Null
        $seen = $false
        for ($i = 0; $i -lt 200; $i++) {
            $pc = (& $PSMUX -L $SOCK capture-pane -p -t $Name 2>$null) -join "`n"
            if ($pc -match 'P735DONE') { $seen = $true; break }
            Start-Sleep -Milliseconds 100
        }
        if (-not $seen) { Write-Host "  (diag) pane $Name never printed its marker in 20 s" }
        Start-Sleep -Milliseconds 500
    }
    & $PSMUX -L $SOCK detach-client -s $Name 2>&1 | Out-Null
    if (-not $cap.WaitForExit(30000)) { Write-Host "  (diag) capture for $Name still running 30 s after detach, killed"; try { Stop-Process -Id $cap.Id -Force } catch {} }
    & $PSMUX -L $SOCK kill-session -t $Name 2>&1 | Out-Null
    if (-not (Test-Path $outBin)) { return $null }
    return [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($outBin))
}

function Cursor-Seqs([string]$Text) {
    $rx = [regex]"\x1b\[[0-9]* q|\x1b\[\?12[hl]"
    return @($rx.Matches($Text) | ForEach-Object { $_.Value.Replace("$ESC", "ESC") })
}

Write-Test "No config: the client writes no cursor shape and no blink at all"
$t = Capture-Case "nocfg" @()
if ($null -eq $t) { Write-Fail "no capture" }
else {
    $s = Cursor-Seqs $t
    if ($s.Count -eq 0) { Write-Pass "no DECSCUSR and no mode 12 in $($t.Length) bytes" }
    else { Write-Fail ("wrote " + ($s -join " ")) }
}

Write-Test "cursor-style bar: a steady bar, then a reset on detach"
$t = Capture-Case "bar" @("set -g cursor-style bar")
if ($null -eq $t) { Write-Fail "no capture" }
else {
    $s = Cursor-Seqs $t
    $i = [array]::IndexOf($s, "ESC[6 q")
    if ($i -ge 0) { Write-Pass "steady bar ESC[6 q written" } else { Write-Fail ("no ESC[6 q: " + ($s -join " ")) }
    $after = if ($i -ge 0) { @($s[($i + 1)..($s.Count)] | Where-Object { $_ -match ' q$' }) } else { @() }
    if ($after.Count -gt 0) { Write-Pass ("shape put back on detach: " + ($after -join " ")) } else { Write-Fail ("no shape restore after the bar: " + ($s -join " ")) }
}

Write-Test "cursor-style blinking-bar (tmux spelling): a blinking bar"
$t = Capture-Case "bbar" @("set -g cursor-style blinking-bar")
if ($null -eq $t) { Write-Fail "no capture" }
else {
    $s = Cursor-Seqs $t
    # The last shape is the detach reset; the one before it is what the
    # session held. Before the fix the frames resolved blinking-bar to ESC[0 q.
    $shapes = @($s | Where-Object { $_ -match ' q$' })
    $held = if ($shapes.Count -ge 2) { $shapes[$shapes.Count - 2] } else { "" }
    if ($held -eq "ESC[5 q") { Write-Pass "the session held ESC[5 q" } else { Write-Fail ("held [$held]: " + ($s -join " ")) }
}

Write-Test "cursor-style block: the steady block tmux means by the word"
$t = Capture-Case "block" @("set -g cursor-style block")
if ($null -eq $t) { Write-Fail "no capture" }
else {
    $s = Cursor-Seqs $t
    if (($s -contains "ESC[2 q") -and -not ($s -contains "ESC[1 q")) { Write-Pass "ESC[2 q written, no blinking block" } else { Write-Fail ("got: " + ($s -join " ")) }
}

Write-Test "A pane program's DECSCUSR still reaches the terminal with no config"
$t = Capture-Case "pane" @() "3"
if ($null -eq $t) { Write-Fail "no capture" }
else {
    $s = Cursor-Seqs $t
    if ($s -contains "ESC[3 q") { Write-Pass ("pane ESC[3 q forwarded: " + ($s -join " ")) } else { Write-Fail ("pane shape lost: " + ($s -join " ")) }
}

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Remove-Item $work -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "RESULT: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped"
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
