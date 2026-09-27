# Issue #702: the copy-mode position indicator in the top right printed the
# scroll offset on both sides of the slash.
#
# Measured on psmux 2c5ee95 before the fix, a 100x30 client, `1..200` echoed
# into the pane, read back off the real console screen with conread.exe:
#
#     entered copy mode   scroll_position=0    history_size=173   indicator=''
#     scrolled up         scroll_position=97   history_size=173   indicator='[97/97]'
#
# and after it:
#
#     entered copy mode   scroll_position=0    history_size=173   indicator='[0/173]'
#     scrolled up         scroll_position=97   history_size=173   indicator='[97/173]'
#     gutter absolute     scroll_position=97   history_size=173   indicator='[77/202]'
#
# tmux 3.7c driven the same way (`tmux -L postest`, 30-row pane, `seq 1 200`)
# reports `#{copy_position}/#{copy_position_limit}` as 0/173 at the live bottom,
# 68/173 scrolled up, and 106/203 for that same view once
# `copy-mode-line-numbers` is `absolute`, which is the rule this pins.
#
# capture-pane cannot see any of this: the indicator is drawn by the client
# around the pane, not by the shell inside it, so the oracle is the real console
# screen buffer of the attached client (tests/conread.cs).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "i702pos" }
$SESSION = "i702_s"
$COLS = 100
$ROWS = 30

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Cyan }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
# A shell running inside psmux hands its child the session markers, and a
# nested client refuses to start, so clear them before launching one.
foreach ($v in 'PSMUX_SESSION','PSMUX_TARGET_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_i702_pos"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"

Write-Host ""
Write-Host "=== Issue #702: the copy-mode position indicator ===" -ForegroundColor Magenta
Write-Info "Binary: $PSMUX"
Write-Info "Namespace: $NS   data: $($env:PSMUX_DATA_DIR)"

function Invoke-Psmux([string[]]$psmuxArgs) { & $PSMUX -L $NS @psmuxArgs 2>&1 }
function Stop-Srv { & $PSMUX -L $NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }

function Exit-Test([int]$code) {
    Stop-Srv
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
    if ($null -ne $savedDataDir) { $env:PSMUX_DATA_DIR = $savedDataDir } else { Remove-Item env:PSMUX_DATA_DIR -EA SilentlyContinue }
    if ($null -ne $savedNoWarm)  { $env:PSMUX_NO_WARM  = $savedNoWarm }  else { Remove-Item env:PSMUX_NO_WARM  -EA SilentlyContinue }
    exit $code
}

# --- the screen oracle ------------------------------------------------------
$CONREAD = Join-Path $root "conread.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (Test-Path $csc) {
    & $csc /nologo /optimize /out:$CONREAD (Join-Path $repoTests "conread.cs") 2>&1 | Out-Null
}
if (-not (Test-Path $CONREAD)) {
    Write-Skip "conread.exe could not be built, so the drawn screen cannot be read"
    Exit-Test 0
}

# --- an attached client with scrollback ------------------------------------
Stop-Srv
$proc = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"new-session","-s",$SESSION,"-x","$COLS","-y","$ROWS" -PassThru
Start-Sleep -Seconds 5
if ($proc.HasExited) {
    Write-Skip "the client exited before it attached, so there is no screen to read"
    Exit-Test 0
}

Invoke-Psmux @('send-keys','-t',$SESSION,'1..200 | ForEach-Object { "line$_" }','Enter') | Out-Null
Start-Sleep -Seconds 3

function Get-Fmt([string]$f) { (Invoke-Psmux @('display-message','-t',$SESSION,'-p',$f) | Out-String).Trim() }

# The drawn indicator, or an empty string when nothing is drawn. Read from the
# client's own console screen buffer.
function Get-Indicator {
    $screen = (& $CONREAD $proc.Id 2>&1 | Out-String)
    $m = [regex]::Match($screen, '\[\d+/\d+\]')
    if ($m.Success) { return $m.Value }
    return ""
}

function Move-ViewUp([int]$n) {
    for ($i = 0; $i -lt $n; $i++) { Invoke-Psmux @('send-keys','-t',$SESSION,'-X','cursor-up') | Out-Null }
    Start-Sleep -Milliseconds 800
}

$probe = Get-Indicator
if ($probe -ne "") {
    Write-Fail "an indicator was already on screen before copy mode: '$probe'"
} else {
    Write-Pass "no indicator outside copy mode"
}

# --- 1. it appears at the live bottom --------------------------------------
Write-Host ""
Write-Host "--- entering copy mode ---" -ForegroundColor Yellow
Invoke-Psmux @('copy-mode','-t',$SESSION) | Out-Null
Start-Sleep -Milliseconds 900
$hist = [int](Get-Fmt '#{history_size}')
$ind = Get-Indicator
Write-Info "history_size=$hist indicator='$ind'"
if ($ind -eq "[0/$hist]") {
    Write-Pass "at the live bottom the indicator reads [0/$hist] (#702 drew nothing here)"
} else {
    Write-Fail "at the live bottom the indicator read '$ind', want '[0/$hist]'"
}

# --- 2. scrolled up, the two numbers differ --------------------------------
Write-Host ""
Write-Host "--- scrolled into the history ---" -ForegroundColor Yellow
Move-ViewUp 125
$scroll = [int](Get-Fmt '#{scroll_position}')
$hist = [int](Get-Fmt '#{history_size}')
$ind = Get-Indicator
Write-Info "scroll_position=$scroll history_size=$hist indicator='$ind'"
if ($scroll -le 0) {
    Write-Skip "the view never left the live bottom on this host, so there is nothing to compare"
} elseif ($ind -eq "[$scroll/$hist]") {
    Write-Pass "the indicator reads [$scroll/$hist], the offset over the scrollback size"
} else {
    Write-Fail "the indicator read '$ind', want '[$scroll/$hist]' (#702 read '[$scroll/$scroll]')"
}
if ($scroll -gt 0 -and $ind -match '^\[(\d+)/(\d+)\]$' -and $Matches[1] -eq $Matches[2]) {
    Write-Fail "both sides of the slash are $($Matches[1]) again, which is the #702 defect"
} else {
    Write-Pass "the two sides of the slash are not the same number"
}

# --- 3. an absolute gutter switches the reading ----------------------------
Write-Host ""
Write-Host "--- copy-mode-line-numbers absolute ---" -ForegroundColor Yellow
$paneRows = [int](Get-Fmt '#{pane_height}')
foreach ($mode in @('absolute','relative','hybrid')) {
    Invoke-Psmux @('set-option','-g','copy-mode-line-numbers',$mode) | Out-Null
    Start-Sleep -Milliseconds 900
    $scroll = [int](Get-Fmt '#{scroll_position}')
    $hist = [int](Get-Fmt '#{history_size}')
    $want = "[{0}/{1}]" -f ($hist + 1 - $scroll), ($hist + $paneRows)
    $ind = Get-Indicator
    if ($ind -eq $want) {
        Write-Pass "$mode gutter: the indicator reads $want, counting from the top of the history"
    } else {
        Write-Fail "$mode gutter: the indicator read '$ind', want '$want'"
    }
}
Invoke-Psmux @('set-option','-g','copy-mode-line-numbers','off') | Out-Null
Start-Sleep -Milliseconds 900
$scroll = [int](Get-Fmt '#{scroll_position}')
$hist = [int](Get-Fmt '#{history_size}')
$ind = Get-Indicator
if ($ind -eq "[$scroll/$hist]") {
    Write-Pass "switching the gutter off returns the offset reading [$scroll/$hist]"
} else {
    Write-Fail "with the gutter off the indicator read '$ind', want '[$scroll/$hist]'"
}

# --- 4. it goes away with copy mode ----------------------------------------
Write-Host ""
Write-Host "--- leaving copy mode ---" -ForegroundColor Yellow
Invoke-Psmux @('send-keys','-t',$SESSION,'-X','cancel') | Out-Null
Start-Sleep -Milliseconds 900
$ind = Get-Indicator
if ($ind -eq "") {
    Write-Pass "the indicator is gone once copy mode closes"
} else {
    Write-Fail "the indicator survived copy mode: '$ind'"
}

Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Magenta
Write-Host "  Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed:  $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { 'Red' } else { 'Green' })
Write-Host "  Skipped: $script:TestsSkipped" -ForegroundColor Yellow
Exit-Test $(if ($script:TestsFailed -gt 0) { 1 } else { 0 })
