# Issue #902: a paste in ConEmu reaches the pane as the text, without the
# bracketed paste markers.
#
# The client asks its terminal for bracketed paste (`ESC[?2004h`). ConEmu has
# no ConPTY: it writes a paste into the real console with WriteConsoleInput,
# and once asked it writes `ESC[200~` and `ESC[201~` around it, as plain key
# records. On the console input route nothing decodes them, so a paste of
# `Hello, psmux!` reached the pane as ESC, `[200~Hello, psmux!`, ESC, `[201~`
# (ConEmu 230724, PSMUX_INPUT_DEBUG=1). conhost does not do this: it brackets
# a paste only while the console has VT input on (Clipboard.cpp:178-179 in
# microsoft/terminal 98c8c795), and the console route turns that off.
#
# The client no longer asks ConEmu (ConEmuPID set) for bracketed paste on the
# console route. The paste then arrives as typed keys, which the client's
# paste detection sends to the pane as one paste. The VT input route still
# asks, because its parser decodes the markers.
#
# Each case starts the client in a new ConEmu window, its own instance with
# default settings (-NoSingle -basic), and pastes with ConEmu's Paste GuiMacro
# (`ConEmuC -GuiMacro:<pid> Paste(2,"text")`), which takes the same path as
# Ctrl+V and needs no focus. The pane runs Windows PowerShell without a
# profile, where ESC clears the line, so before the fix the pane is left
# holding `[201~`. It opens windows, so do not type while it runs. Skipped
# when ConEmu is not installed.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:TestsSkipped = 0
$script:OpenedPids = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$conemu = $null
foreach ($cand in @("$env:ProgramFiles\ConEmu\ConEmu64.exe", "${env:ProgramFiles(x86)}\ConEmu\ConEmu64.exe")) {
    if ($cand -and (Test-Path $cand)) { $conemu = $cand; break }
}
if (-not $conemu) {
    $c = Get-Command ConEmu64 -EA SilentlyContinue
    if ($c) { $conemu = $c.Source }
}
if (-not $conemu) {
    Write-Skip "ConEmu is not installed"
    Write-Host ""
    Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed  skipped: $script:TestsSkipped" -ForegroundColor Cyan
    exit $script:TestsFailed
}
$conemuc = Join-Path (Split-Path $conemu) "ConEmu\ConEmuC64.exe"
Write-Info "conemu: $conemu"

$tag = [guid]::NewGuid().ToString('N').Substring(0, 6)
$NS = "cemu-$tag"; $S = "cemu"
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cemu_" + $tag)
New-Item -ItemType Directory -Force $TMP | Out-Null
$DATA = Join-Path $TMP "data"
New-Item -ItemType Directory -Force $DATA | Out-Null
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
$conf = Join-Path $TMP "cemu.conf"
# No profile, so the prompt is the stock `PS <path>>` and ESC is PSReadLine's
# RevertLine whatever the user's profile binds.
(@("set -g default-shell '$POWERSHELL -NoLogo -NoProfile'", "set -g status-left ''") -join "`n") + "`n" |
    Set-Content $conf -Encoding ASCII

function Kill-Ns {
    $env:PSMUX_DATA_DIR = $DATA
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
    Start-Sleep -Milliseconds 600
}

function Stop-Opened {
    foreach ($id in $script:OpenedPids) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:OpenedPids = @()
}

function Psmux {
    $env:PSMUX_DATA_DIR = $DATA
    $out = & $PSMUX -L $NS @args 2>&1 | Out-String
    Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
    return $out
}

# Start the client in a new ConEmu window, paste `$text` there, and return the
# pane's last prompt line, or $null when the client never came up.
function Paste-In-ConEmu($text, [bool]$vtInput) {
    Kill-Ns
    $launch = Join-Path $TMP "launch.ps1"
    @(
        # What the client must not take from the shell running this suite: a
        # psmux session around it would make it refuse to nest, and another
        # terminal's signal would decide its input route.
        "Get-ChildItem Env: | Where-Object { `$_.Name -like 'PSMUX_*' -or `$_.Name -like 'TMUX*' } | ForEach-Object { Remove-Item `"Env:`$(`$_.Name)`" }",
        "Remove-Item Env:\TERM_PROGRAM,Env:\WEZTERM_PANE,Env:\TERMINAL_EMULATOR,Env:\ALACRITTY_LOG -EA SilentlyContinue",
        "`$env:PSMUX_DATA_DIR = '$DATA'",
        "`$env:PSMUX_NO_WARM = '1'",
        $(if ($vtInput) { "`$env:PSMUX_VT_INPUT = '1'" } else { "" }),
        "Set-Location `"`$env:SystemDrive\`"",
        "& '$PSMUX' -L $NS -f '$conf' new-session -s $S"
    ) | Set-Content -Path $launch -Encoding UTF8
    $run = "pwsh -NoProfile -ExecutionPolicy Bypass -File `"$launch`""
    $p = Start-Process -FilePath $conemu -PassThru -ArgumentList "-NoSingle -basic -NoUpdate -nokeyhooks -run $run"
    $script:OpenedPids += $p.Id
    $up = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 300
        $null = Psmux has-session -t $S
        if ($LASTEXITCODE -eq 0) { $up = $true; break }
    }
    if (-not $up) { Stop-Opened; return $null }
    # The prompt has to be drawn before the paste lands after it.
    Start-Sleep -Seconds 5
    $m = Start-Process -FilePath $conemuc -PassThru -Wait -NoNewWindow `
        -ArgumentList "-Silent -GuiMacro:$($p.Id) Paste(2,`"$text`")"
    if ($m.ExitCode -ne 0) { Write-Info "GuiMacro exit $($m.ExitCode)" }
    Start-Sleep -Seconds 2
    $cap = Psmux capture-pane -t $S -p
    $prompt = ($cap -split "`n" | Where-Object { $_ -match '^PS .*>' } | Select-Object -Last 1)
    Kill-Ns
    Start-Sleep -Milliseconds 800
    Stop-Opened
    if ($prompt) { return $prompt.TrimEnd() }
    Write-Info "the pane held [$($cap.Trim())]"
    return ""
}

$text = "Hello, psmux!"

Write-Head "1. a paste on the console route reaches the pane as the text"
$line = Paste-In-ConEmu $text $false
Write-Info "prompt line [$line]"
if ($null -eq $line) {
    Write-Fail "the client never came up in ConEmu"
} else {
    Check ($line -notmatch '20[01]~') "no paste marker reached the pane" "the pane holds a paste marker: [$line]"
    Check ($line.EndsWith("> $text")) "the pane holds the pasted text" "the pane does not end with the pasted text: [$line]"
}

# The VT input route asks for the markers and its parser decodes them. The
# same on every build, and it shows that ConEmu really writes the markers when
# asked: PSMUX_VT_INPUT=1 is the route a user could take before the fix.
Write-Head "2. with PSMUX_VT_INPUT=1 the markers are decoded"
$line = Paste-In-ConEmu $text $true
Write-Info "prompt line [$line]"
if ($null -eq $line) {
    Write-Fail "the client never came up in ConEmu"
} else {
    Check (($line -notmatch '20[01]~') -and $line.EndsWith("> $text")) "the VT route pasted the text alone" `
        "the VT route left [$line]"
}

# -- Done --

Kill-Ns
Stop-Opened
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed  skipped: $script:TestsSkipped" -ForegroundColor Cyan
exit $script:TestsFailed
