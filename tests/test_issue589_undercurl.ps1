# Issue #589: "Undercurl style is not supported".
#
# Reported against psmux 3.3.8 under Windows Terminal 1.24: `ESC[4:2m`,
# `ESC[4:3m`, `ESC[4:4m` and `ESC[4:5m` render with the right underline style
# in plain Windows Terminal but come out with NO underline at all inside a
# psmux pane, and `ESC[58...m` underline colour is dropped as well.
#
# Root cause: crates/vt100-psmux/src/screen.rs matched only the bare `[4]`
# SGR parameter, so every subparameter form fell through to `unhandled` and
# the cell kept no underline. Windows ConPTY forwards `4:3`, `4:4`, `4:5` and
# the COLON forms of SGR 58 verbatim, and rewrites `4:2` into the legacy
# SGR 21, so all of them reach psmux and all of them were dropped.
#
# tmux parity: tmux 3.4 input.c input_csi_dispatch_sgr_colon handles p[0] == 4
# with subparameters 0..5 (GRID_ATTR_UNDERSCORE..UNDERSCORE_5), case 21 maps
# onto UNDERSCORE_2, and p[0] == 58 sets gc->us. tmux's capture-pane -e
# re-encodes them as `4:2`..`4:5` plus `58;2;r;g;b` (grid.c
# grid_string_cells_code), which is exactly what is asserted below.
#
# NOTE on NO_COLOR: with NO_COLOR set in the environment, PowerShell 7 switches
# $PSStyle.OutputRendering to PlainText and strips the SGR sequences it
# recognises out of Write-Host output. Its matcher does NOT recognise the colon
# subparameter forms, so the pane would receive `4:3m` and `58:5:9m` with every
# surrounding `ESC[0m` and `ESC[24m` deleted, and the style would legitimately
# run on to the end of the screen. That is a stripped payload, not a psmux
# fault (real tmux 3.4 renders the same bytes the same way), so this script
# clears NO_COLOR for the session it creates AND the payload forces
# OutputRendering back to Ansi. Test 0 below fails loudly if anything strips
# them anyway, instead of letting it look like a psmux regression.
#
# NOTE on SGR 58: the Windows SYSTEM conhost that backs every psmux pane does
# NOT understand the SEMICOLON form `58;2;r;g;b`. It skips the 58 and then
# reads 2/255/0/0 as ordinary SGR parameters, so the trailing 0 resets every
# attribute before psmux ever sees a colour. The COLON forms
# `58:2::r:g:b` and `58:5:n` (what nvim, helix, kitty and tmux's own Setulc
# capability emit) pass through untouched. This test therefore uses the colon
# form for the coloured case; the semicolon form is covered by the Rust test
# crates/vt100-psmux/tests/issue589_undercurl.rs.

$ErrorActionPreference = "Continue"
# Both of these are inherited by the server and by every pane it spawns.
Remove-Item Env:\NO_COLOR -EA SilentlyContinue
$env:PSMUX_NO_WARM = "1"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SOCK = "i589"
$SESSION = "t589"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$emptyConf = "$env:TEMP\psmux_589_empty.conf"
"" | Set-Content -Path $emptyConf -Encoding UTF8

# The payload the pane runs. Deliberately the reporter's own recipe: plain
# Write-Host lines with the SGR 4 subparameter forms.
$payload = "$env:TEMP\psmux_589_payload.ps1"
@'
# Never let the host strip what this script is deliberately writing.
Remove-Item Env:\NO_COLOR -EA SilentlyContinue
if ($PSStyle) { $PSStyle.OutputRendering = 'Ansi' }
# Let ConPTY flush its own frame preamble (ESC[2J ESC[m ESC[H, the title OSC,
# ESC[?25h) before the first styled write, so the preamble's reset can never
# land after the first line's attributes.
Start-Sleep -Milliseconds 400
Write-Host "WARMUP_LINE"
$e = [char]27
Write-Host "$e[4;1mSTD_UL$e[0m"
Write-Host "$e[4:1mEXT_SINGLE$e[0m"
Write-Host "$e[4:2mDOUBLE_UL$e[0m"
Write-Host "$e[4:3mCURLY_UL$e[0m"
Write-Host "$e[4:4mDOTTED_UL$e[0m"
Write-Host "$e[4:5mDASHED_UL$e[0m"
Write-Host "$e[4:3m$e[58:2::255:0:0mCOLOR_CURLY$e[0m"
Write-Host "$e[4:3m$e[58:5:9mIDX_CURLY$e[0m"
Write-Host "$e[4:3mBLEED_ON$e[24mBLEED_OFF"
Write-Host "$e[4:3mCOLON0_ON$e[4:0mCOLON0_OFF"
Write-Host "AFTER_ALL"
Start-Sleep -Seconds 600
'@ | Set-Content -Path $payload -Encoding UTF8

function Cleanup {
    & $PSMUX -L $SOCK kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\${SOCK}__*" -Force -EA SilentlyContinue
}

function Start-PayloadSession($name) {
    & $PSMUX -L $SOCK kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    & $PSMUX -L $SOCK -f $emptyConf new-session -d -s $name -x 100 -y 30 `
        "pwsh -NoProfile -NoLogo -File $payload" | Out-Null
    Start-Sleep -Seconds 5
}

function Get-StyledCapture($name) {
    (& $PSMUX -L $SOCK capture-pane -t $name -p -e 2>&1 | Out-String)
}

function Get-LineFor($capture, $label) {
    ($capture -split "`r?`n" | Where-Object { $_ -match [regex]::Escape($label) } | Select-Object -First 1)
}

Write-Host "`n=== Issue #589 Tests: styled underscores and underline colour ===" -ForegroundColor Cyan

# === TEST 0: the payload's own escapes reached the pane ===
# Guard, not a psmux assertion. If the host stripped the semicolon SGRs on the
# way out, every later result is meaningless, so say so plainly here rather
# than letting it read as a psmux failure.
Write-Host "`n[Test 0] precondition: the payload's SGR sequences were not stripped" -ForegroundColor Yellow
Cleanup
Start-PayloadSession $SESSION
$cap = Get-StyledCapture $SESSION
$stdLine = Get-LineFor $cap "STD_UL"
$extLine = Get-LineFor $cap "EXT_SINGLE"
if ($stdLine -notmatch '4' -and $extLine -match '4') {
    Write-Fail ("the shell stripped the semicolon SGR forms from the payload " +
        "(NO_COLOR set, or `$PSStyle.OutputRendering is PlainText). " +
        "STD_UL=$($stdLine -replace "`e", '<ESC>'). Nothing below is a psmux result.")
} else {
    Write-Pass "payload escapes reached the pane intact"
}

# === TEST 1: capture-pane -e re-encodes every style, tmux style ===
Write-Host "`n[Test 1] capture-pane -e carries 4:2 / 4:3 / 4:4 / 4:5" -ForegroundColor Yellow

foreach ($case in @(
    @{ Label = "DOUBLE_UL"; Want = "4:2" },
    @{ Label = "CURLY_UL";  Want = "4:3" },
    @{ Label = "DOTTED_UL"; Want = "4:4" },
    @{ Label = "DASHED_UL"; Want = "4:5" }
)) {
    $line = Get-LineFor $cap $case.Label
    if ($null -eq $line) { Write-Fail "$($case.Label): line missing from capture"; continue }
    $vis = $line -replace "`e", '<ESC>'
    if ($line -match [regex]::Escape($case.Want)) { Write-Pass "$($case.Label) -> $($case.Want)   $vis" }
    else { Write-Fail "$($case.Label) lost its style, expected $($case.Want): $vis" }
}

# === TEST 2: plain SGR 4 and 4:1 stay plain (no regression) ===
Write-Host "`n[Test 2] SGR 4 and 4:1 stay a plain single underline" -ForegroundColor Yellow
foreach ($label in @("STD_UL", "EXT_SINGLE")) {
    $line = Get-LineFor $cap $label
    $vis = $line -replace "`e", '<ESC>'
    if ($null -eq $line) { Write-Fail "${label}: line missing"; continue }
    if ($line -match '4(;|m)' -and $line -notmatch '4:') { Write-Pass "$label stayed plain: $vis" }
    else { Write-Fail "$label is not a plain underline: $vis" }
}
# `4;1` is underline PLUS bold, never undercurl.
$std = Get-LineFor $cap "STD_UL"
if ($std -match '1' -and $std -match '4') { Write-Pass "4;1 kept both bold and underline" }
else { Write-Fail "4;1 semantics changed: $($std -replace "`e", '<ESC>')" }

# === TEST 3: underline colour survives, both forms ===
Write-Host "`n[Test 3] SGR 58 underline colour reaches capture-pane -e" -ForegroundColor Yellow
$line = Get-LineFor $cap "COLOR_CURLY"
$vis = $line -replace "`e", '<ESC>'
if ($line -match '4:3' -and $line -match '58;2;255;0;0') { Write-Pass "truecolour undercurl: $vis" }
else { Write-Fail "truecolour undercurl lost: $vis" }
$line = Get-LineFor $cap "IDX_CURLY"
$vis = $line -replace "`e", '<ESC>'
if ($line -match '4:3' -and $line -match '58;5;9') { Write-Pass "indexed undercurl: $vis" }
else { Write-Fail "indexed undercurl lost: $vis" }

# === TEST 4: no bleed past SGR 24, SGR 4:0 or SGR 0 ===
Write-Host "`n[Test 4] the style never bleeds into the text that follows" -ForegroundColor Yellow
$line = Get-LineFor $cap "BLEED_OFF"
$vis = $line -replace "`e", '<ESC>'
# Everything after BLEED_OFF on that line must not be styled: the last SGR
# before the label has to be a reset, not another 4:3.
$tail = $line -replace '^.*BLEED_OFF', ''
$head = $line -replace 'BLEED_OFF.*$', ''
if ($head -match '4:3') { Write-Pass "BLEED_ON kept its curl: $vis" }
else { Write-Fail "BLEED_ON lost its curl: $vis" }
if ($tail -notmatch '4:') { Write-Pass "SGR 24 stopped the curl before BLEED_OFF" }
else { Write-Fail "curl bled past SGR 24: $vis" }

$line = Get-LineFor $cap "COLON0_OFF"
$vis = $line -replace "`e", '<ESC>'
$tail = $line -replace '^.*COLON0_OFF', ''
if ($tail -notmatch '4:') { Write-Pass "SGR 4:0 stopped the curl" }
else { Write-Fail "curl bled past SGR 4:0: $vis" }

$line = Get-LineFor $cap "AFTER_ALL"
$vis = $line -replace "`e", '<ESC>'
# Only the underline STYLE is asserted across lines. ConPTY models underlines,
# so it re-emits the reset faithfully; it does NOT model the SGR 58 colour and
# forwards it blind, so where that colour lands relative to a reset is a
# property of the shell and of ConPTY, not of psmux. The cross-line colour
# reset is pinned deterministically in
# crates/vt100-psmux/tests/issue589_undercurl.rs instead.
if ($line -notmatch '4:') { Write-Pass "AFTER_ALL carries no underline style: $vis" }
else { Write-Fail "underline style bled onto AFTER_ALL: $vis" }

# === TEST 5: the run JSON the client renders from carries ul / ulc ===
Write-Host "`n[Test 5] dump-state run JSON carries ul and ulc" -ForegroundColor Yellow
$dump = (& $PSMUX -L $SOCK dump-state -t $SESSION 2>&1 | Out-String)
foreach ($case in @(
    @{ Label = "DOUBLE_UL"; Want = '"ul":2' },
    @{ Label = "CURLY_UL";  Want = '"ul":3' },
    @{ Label = "DOTTED_UL"; Want = '"ul":4' },
    @{ Label = "DASHED_UL"; Want = '"ul":5' }
)) {
    $run = [regex]::Match($dump, '\{"text":"' + $case.Label + '[^}]*\}').Value
    if ($run -and $run.Contains($case.Want)) { Write-Pass "$($case.Label): $run" }
    else { Write-Fail "$($case.Label) run missing $($case.Want): $run" }
}
$run = [regex]::Match($dump, '\{"text":"COLOR_CURLY[^}]*\}').Value
if ($run -match '"ul":3' -and $run -match '"ulc":"rgb:255,0,0"') { Write-Pass "COLOR_CURLY: $run" }
else { Write-Fail "COLOR_CURLY run missing ul/ulc: $run" }
$run = [regex]::Match($dump, '\{"text":"IDX_CURLY[^}]*\}').Value
if ($run -match '"ul":3' -and $run -match '"ulc":"idx:9"') { Write-Pass "IDX_CURLY: $run" }
else { Write-Fail "IDX_CURLY run missing ul/ulc: $run" }
$run = [regex]::Match($dump, '\{"text":"AFTER_ALL[^}]*\}').Value
# See the note above: "ulc" is deliberately not asserted here.
if ($run -and $run -notmatch '"ul":') { Write-Pass "AFTER_ALL run has no underline style" }
else { Write-Fail "AFTER_ALL run carries an underline style: $run" }

# === TEST 6: FLAG_UNDERLINE stays set so older clients still underline ===
Write-Host "`n[Test 6] flags keeps bit 8 for every styled run" -ForegroundColor Yellow
$ok = $true
foreach ($label in @("DOUBLE_UL", "CURLY_UL", "DOTTED_UL", "DASHED_UL")) {
    $run = [regex]::Match($dump, '\{"text":"' + $label + '[^}]*\}').Value
    $m = [regex]::Match($run, '"flags":(\d+)')
    if (-not $m.Success -or (([int]$m.Groups[1].Value) -band 8) -eq 0) {
        Write-Fail "${label} lost FLAG_UNDERLINE: $run"; $ok = $false
    }
}
if ($ok) { Write-Pass "every styled run still sets FLAG_UNDERLINE" }

# ============================================================
# Win32 TUI VISUAL VERIFICATION (attached client)
# ============================================================
Write-Host "`n=== Win32 TUI verification ===" -ForegroundColor Cyan
Cleanup
$TUISESS = "t589tui"
& $PSMUX -L $SOCK kill-session -t $TUISESS 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

# 6a. A real attached client must not crash or blank the pane, and the pane
#     content must still carry the styles while a client is drawing it.
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $TUISESS -x 100 -y 30 `
    "pwsh -NoProfile -NoLogo -File $payload" | Out-Null
Start-Sleep -Seconds 5
$proc = Start-Process -FilePath $PSMUX -ArgumentList "-L",$SOCK,"attach","-t",$TUISESS -PassThru
Start-Sleep -Seconds 4
$capTui = Get-StyledCapture $TUISESS
$line = Get-LineFor $capTui "CURLY_UL"
$vis = $line -replace "`e", '<ESC>'
if ($line -match '4:3') { Write-Pass "TUI: attached client, pane still carries 4:3   $vis" }
else { Write-Fail "TUI: style lost while attached: $vis" }
$line = Get-LineFor $capTui "AFTER_ALL"
if ($line -and $line -notmatch '4:') { Write-Pass "TUI: AFTER_ALL still clean while attached" }
else { Write-Fail "TUI: AFTER_ALL styled while attached: $($line -replace "`e", '<ESC>')" }

# 6b. The bytes the CLIENT writes to its outer terminal. This is the surface
#     the reporter actually sees, so it is checked byte-exactly by hosting the
#     attached client inside a CreatePseudoConsole (tests/conptycap.cs) and
#     grepping the raw stream. Skipped, not failed, when csc.exe is absent.
$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$capSrc = Join-Path $repoTests "conptycap.cs"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$capExe = "$env:TEMP\psmux589_conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}
if (Test-Path $capExe) {
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 800
    # The attached client has to be launched from a shell with no psmux
    # environment, otherwise it refuses with "sessions should be nested with
    # care".
    $launch = "$env:TEMP\psmux589_attach.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $SOCK attach -t $TUISESS
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = "$env:TEMP\psmux589_client.bin"
    Remove-Item $outBin -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "9000"
    Start-Process -FilePath $capExe -ArgumentList @($outBin,"100","30","8",$launch) -Wait -WindowStyle Minimized
    if (Test-Path $outBin) {
        $bytes = [System.IO.File]::ReadAllBytes($outBin)
        $text = [System.Text.Encoding]::ASCII.GetString($bytes)
        # No passthrough mode before Windows 11: the inbox pseudo console of
        # Windows 10 (19045) re-renders what the client wrote, and it has no
        # underline styles, so 4:2 to 4:5 cannot come out of it whatever the
        # client wrote (the pane checks above, from psmux's own grid, pass
        # there).
        $NoUlStyles = [Environment]::OSVersion.Version.Build -lt 22000
        foreach ($want in @("4:3", "4:4", "4:5")) {
            if ($text.Contains($want)) { Write-Pass "TUI: client wrote $want to its terminal" }
            elseif ($NoUlStyles) { Write-Skip "TUI: $want, the Windows 10 pseudo console re-renders without underline styles" }
            else { Write-Fail "TUI: client never wrote $want (this is the reported bug)" }
        }
        # `4:2` reaches the outer terminal as the legacy 21 when an outer
        # conhost re-renders, so accept either spelling.
        if ($text.Contains("4:2") -or $text.Contains("$([char]27)[21m")) {
            Write-Pass "TUI: client wrote a double underline"
        } elseif ($NoUlStyles) { Write-Skip "TUI: double underline, the Windows 10 pseudo console re-renders without underline styles"
        } else { Write-Fail "TUI: client never wrote a double underline" }
        # crossterm's semicolon form of SGR 58 is mis-parsed by conhost into
        # blink plus strikethrough, so psmux must use the colon form.
        if ($text -notmatch '58;') { Write-Pass "TUI: client used the colon form of SGR 58" }
        else { Write-Fail "TUI: client emitted the semicolon form of SGR 58" }
    } else { Write-Skip "conptycap produced no output" }
} else {
    Write-Skip "csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
}

& $PSMUX -L $SOCK kill-session -t $TUISESS 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# === TEARDOWN ===
Cleanup
Remove-Item $payload -Force -EA SilentlyContinue

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
