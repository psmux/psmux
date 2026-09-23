# Issue #687: a keyboard selection in copy mode copies a different range than
# the one on screen once the view has been scrolled.
#
# b957aa6 gave the selection endpoint its own scroll offset so that a mouse drag
# reaching a pane edge keeps the offset the endpoint was measured at.  Only the
# nine mouse handlers ever wrote that offset.  Every keyboard route left it at
# 0, so the endpoint was resolved against the live bottom of the buffer while
# the anchor was resolved against the scrolled view, and the copied range came
# out displaced by the scroll offset.
#
# Layers: the send-keys -X verbs (what a binding runs), the vi keys themselves,
#         character mode, and a second copy-mode session after a pinned one.
#
# Each part selects three known lines and asserts the buffer holds exactly
# those three.  Run before the fix and every scrolled case fails.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE }
         elseif ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE }
         else { (Get-Command psmux -EA Stop).Source }

$tmp = Join-Path $env:TEMP "psmux_copysel"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
$env:PSMUX_NO_WARM = "1"

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

Write-Host "`n=== Copy mode: a keyboard selection copies what it paints ===" -ForegroundColor Cyan
Write-Info "psmux under test: $PSMUX"
& $PSMUX -V

# The pane child: 200 numbered lines, then a long sleep so the screen holds
# still.  A 24 row pane shows LINE178..LINE200 on rows 0..22 and an empty row
# 23, which is where copy mode parks its cursor.
$bash = $null
foreach ($cand in @("C:\Program Files\Git\bin\bash.exe",
                    "C:\Program Files (x86)\Git\bin\bash.exe",
                    "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe")) {
    if (Test-Path $cand) { $bash = $cand; break }
}
if (-not $bash) {
    $bcmd = Get-Command bash -EA SilentlyContinue
    if ($bcmd) { $bash = $bcmd.Source }
}
if (-not $bash) { Write-Fail "no bash found, cannot build the pane child"; exit 1 }

$fill = Join-Path $tmp "fill.sh"
"for i in `$(seq 1 200); do printf 'LINE%03d\n' `$i; done; sleep 120" `
    -replace "`r`n", "`n" | Set-Content $fill -Encoding ASCII -NoNewline
$fillPosix = "/" + (($fill -replace '\\', '/') -replace ':', '')
$child = Join-Path $tmp "child.cmd"
"@echo off`r`n`"$bash`" -c `"sh $fillPosix`"`r`n" | Set-Content $child -Encoding ASCII

$NS = "cpsel"

function Stop-Ns {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    & $PSMUX -L "$($NS)____warm__" kill-server 2>&1 | Out-Null
}

function Start-Ns {
    Stop-Ns
    & $PSMUX -L $NS new-session -d -s s -x 80 -y 24 $child 2>&1 | Out-Null
    Start-Sleep -Milliseconds 2200
    & $PSMUX -L $NS copy-mode -t s 2>&1 | Out-Null
    # Park the cursor on LINE196 (row 18 of 24), clear of both edges so no
    # motion below is clamped.
    1..5 | ForEach-Object { & $PSMUX -L $NS send-keys -t s -X cursor-up 2>&1 | Out-Null }
}

function Send-X($verb, $times = 1) {
    1..$times | ForEach-Object { & $PSMUX -L $NS send-keys -t s -X $verb 2>&1 | Out-Null }
}

function Send-Key($k, $times = 1) {
    1..$times | ForEach-Object { & $PSMUX -L $NS send-keys -t s $k 2>&1 | Out-Null }
}

function Get-Buffer {
    Start-Sleep -Milliseconds 250
    ((& $PSMUX -L $NS show-buffer 2>&1 | Out-String).TrimEnd()) -replace "`r?`n", ","
}

# The cursor sits on LINE196 before any scrolling.  Scrolling the view up by N
# leaves the cursor on the same screen row, which is now LINE(196-N), and the
# two rows below it are the next two numbers.
function Expected($scroll) {
    $first = 196 - $scroll
    (($first..($first + 2)) | ForEach-Object { "LINE{0:D3}" -f $_ }) -join ","
}

Write-Host "`n[Part A] the send-keys -X verbs, which is what a binding runs" -ForegroundColor Yellow
foreach ($scroll in @(0, 3, 10)) {
    Start-Ns
    if ($scroll -gt 0) { Send-X "scroll-up" $scroll }
    Send-X "select-line"
    Send-X "cursor-down" 2
    Send-X "copy-selection"
    $got = Get-Buffer
    $want = Expected $scroll
    if ($got -eq $want) { Write-Pass "scroll-up x$scroll copied [$got]" }
    else { Write-Fail "scroll-up x$scroll copied [$got], want [$want]" }
    Stop-Ns
}

Write-Host "`n[Part B] the vi keys themselves" -ForegroundColor Yellow
foreach ($scroll in @(0, 3, 10)) {
    Start-Ns
    if ($scroll -gt 0) { Send-X "scroll-up" $scroll }
    Send-Key "V"
    Send-Key "j" 2
    Send-Key "Enter"
    $got = Get-Buffer
    $want = Expected $scroll
    if ($got -eq $want) { Write-Pass "V, j, j, Enter after scroll-up x$scroll copied [$got]" }
    else { Write-Fail "V, j, j, Enter after scroll-up x$scroll copied [$got], want [$want]" }
    Stop-Ns
}

Write-Host "`n[Part C] character mode after scrolling" -ForegroundColor Yellow
Start-Ns
Send-X "scroll-up" 4
Send-Key "v"
Send-Key "j"
Send-Key "Enter"
$got = Get-Buffer
# From column 0 of LINE192 to column 0 of LINE193: the first line entire, then
# one cell of the second.
$want = "LINE192,L"
if ($got -eq $want) { Write-Pass "v, j, Enter copied [$got]" }
else { Write-Fail "v, j, Enter copied [$got], want [$want]" }
Stop-Ns

Write-Host "`n[Part D] a second copy-mode session starts clean" -ForegroundColor Yellow
Start-Ns
Send-X "scroll-up" 6
Send-X "select-line"
Send-X "cursor-down" 2
Send-X "copy-selection-and-cancel"
Get-Buffer | Out-Null
& $PSMUX -L $NS copy-mode -t s 2>&1 | Out-Null
1..5 | ForEach-Object { & $PSMUX -L $NS send-keys -t s -X cursor-up 2>&1 | Out-Null }
Send-X "scroll-up" 2
Send-X "select-line"
Send-X "cursor-down" 2
Send-X "copy-selection"
$got = Get-Buffer
$want = Expected 2
if ($got -eq $want) { Write-Pass "the second selection copied [$got]" }
else { Write-Fail "the second selection copied [$got], want [$want]" }
Stop-Ns

Write-Host "`n[Part E] no scroll key at all, just the cursor leaving the top row" -ForegroundColor Yellow
# Copy mode parks the cursor on row 23 of 24.  Twenty three cursor-ups reach the
# top row and every one after that scrolls the view by a line, which is what `k`
# and Up do and where the mouse wheel ends up.  Thirty of them leave the view
# seven lines up with the cursor on row 0, so the three selected lines are
# LINE171 to LINE173.
Start-Ns
Send-X "cursor-up" 25   # five in Start-Ns, so thirty in total
Send-Key "V"
Send-Key "j" 2
Send-Key "Enter"
$got = Get-Buffer
$want = "LINE171,LINE172,LINE173"
if ($got -eq $want) { Write-Pass "cursor-up past the top row copied [$got]" }
else { Write-Fail "cursor-up past the top row copied [$got], want [$want]" }
Stop-Ns

Write-Host "`n=== Summary ===" -ForegroundColor Cyan
Write-Host "  Passed:  $script:TestsPassed"
Write-Host "  Failed:  $script:TestsFailed"
Remove-Item $tmp -Recurse -Force -EA SilentlyContinue
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
