# default-shell naming a program whose path has a space, unquoted, with
# arguments after it: `C:\Program Files\PowerShell\7\pwsh.exe -NoLogo`.
# psmux cut the path at its first space and spawned `C:\Program`, so the
# session never started ("CreateProcessW ... The system cannot find the file
# specified"). tmux takes default-shell as one path; psmux also takes
# arguments after the program.
#
# The shell is a copy of cmd.exe in a folder whose name has a space, so the
# suite needs no Git or PowerShell under Program Files. Its arguments
# (`/k echo <mark>`) print a mark, which proves they reached the program.
#
# Private PSMUX_DATA_DIR and config, a -L namespace per case; cleanup is
# `-L <ns> kill-server` only. PSMUX_TEST_BIN (or PSMUX_EXE) picks the binary.

$ErrorActionPreference = "Continue"
foreach ($name in @(Get-ChildItem Env: | Where-Object { ($_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_BIN') -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    [Environment]::SetEnvironmentVariable($name, $null)
}
[Environment]::SetEnvironmentVariable('SHELL', $null)
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_dsp_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$ShellDir = Join-Path $TMP "Program Files\Spaced Shell"
New-Item -ItemType Directory -Force $ShellDir | Out-Null
$Shell = Join-Path $ShellDir "cmd.exe"
Copy-Item "$env:SystemRoot\System32\cmd.exe" $Shell
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"

$script:ns = ""
function P { & $PSMUX -L $script:ns @args 2>&1 }
# A fresh namespace, data dir and config holding $confLines.
function Start-Case($confLines) {
    $script:ns = "dsp_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $data = Join-Path $TMP $script:ns
    New-Item -ItemType Directory -Force $data | Out-Null
    $env:PSMUX_DATA_DIR = $data
    $conf = Join-Path $data "c.conf"
    (@($confLines) + "set -g status off") -join "`n" | Set-Content $conf -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $conf
}
function Stop-Case {
    if ($script:ns) { & $PSMUX -L $script:ns kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    $script:ns = ""
}
function WaitSession {
    for ($i = 0; $i -lt 40; $i++) {
        & $PSMUX -L $script:ns has-session -t A 2>$null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
# The pane's screen holds $mark within a few seconds.
function ShowsMark($target, $mark) {
    for ($i = 0; $i -lt 20; $i++) {
        if (((P capture-pane -p -t $target) -join "`n") -match $mark) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

Write-Host "`n=== default-shell: a spaced program path with arguments ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  Shell:  $Shell"

try {
    Write-Host "`n[1] config: unquoted spaced path with arguments" -ForegroundColor Yellow
    Start-Case "set -g default-shell '$Shell /k echo MARK_CONF'"
    $out = (P new-session -d -s A -x 100 -y 25 | Out-String).Trim()
    if (WaitSession) { Write-Pass "the session started" } else { Write-Fail "the session should start: $out" }
    if (ShowsMark 'A:0.0' 'MARK_CONF') { Write-Pass "new-session ran the program with its arguments" } else { Write-Fail "new-session pane should print MARK_CONF:`n$((P capture-pane -p -t A:0.0) -join "`n")" }
    P new-window -t A | Out-Null
    if (ShowsMark 'A:1.0' 'MARK_CONF') { Write-Pass "new-window ran it too" } else { Write-Fail "new-window pane should print MARK_CONF" }
    P split-window -t A:1 | Out-Null
    if (ShowsMark 'A:1.1' 'MARK_CONF') { Write-Pass "split-window ran it too" } else { Write-Fail "split-window pane should print MARK_CONF" }
    $name = (P display-message -p -t A:0 '#{window_name}' | Out-String).Trim()
    if ($name -eq 'cmd') { Write-Pass "the window is named after the program" } else { Write-Fail "the window name should be cmd, got '$name'" }
    Stop-Case

    Write-Host "`n[2] set-option at runtime" -ForegroundColor Yellow
    Start-Case @()
    P new-session -d -s A -x 100 -y 25 | Out-Null
    if (WaitSession) {
        P set-option -g default-shell "$Shell /k echo MARK_SET" | Out-Null
        P new-window -t A | Out-Null
        if (ShowsMark 'A:1.0' 'MARK_SET') { Write-Pass "new-window ran the program set at runtime" } else { Write-Fail "new-window pane should print MARK_SET:`n$((P capture-pane -p -t A:1.0) -join "`n")" }
        P split-window -t A:1 | Out-Null
        if (ShowsMark 'A:1.1' 'MARK_SET') { Write-Pass "split-window ran it too" } else { Write-Fail "split-window pane should print MARK_SET" }
    } else { Write-Fail "the session should start" }
    Stop-Case

    Write-Host "`n[3] the forms that already worked still work" -ForegroundColor Yellow
    Start-Case "set -g default-shell '`"$Shell`" /k echo MARK_QUOTED'"
    P new-session -d -s A -x 100 -y 25 | Out-Null
    if ((WaitSession) -and (ShowsMark 'A:0.0' 'MARK_QUOTED')) { Write-Pass "a quoted program with arguments" } else { Write-Fail "a quoted program with arguments should run" }
    Stop-Case
    Start-Case "set -g default-shell '$Shell'"
    P new-session -d -s A -x 100 -y 25 | Out-Null
    $cmdName = (P display-message -p -t A:0.0 '#{pane_current_command}' | Out-String).Trim()
    if ((WaitSession) -and $cmdName -eq 'cmd') { Write-Pass "a spaced path alone" } else { Write-Fail "a spaced path alone should run cmd, got '$cmdName'" }
    Stop-Case
    Start-Case "set -g default-shell 'cmd.exe /k echo MARK_BARE'"
    P new-session -d -s A -x 100 -y 25 | Out-Null
    if ((WaitSession) -and (ShowsMark 'A:0.0' 'MARK_BARE')) { Write-Pass "a bare name with arguments" } else { Write-Fail "a bare name with arguments should run" }
    Stop-Case
    Start-Case @()
    $env:SHELL = $Shell
    P new-session -d -s A -x 100 -y 25 | Out-Null
    $cmdName = (P display-message -p -t A:0.0 '#{pane_current_command}' | Out-String).Trim()
    [Environment]::SetEnvironmentVariable('SHELL', $null)
    if ((WaitSession) -and $cmdName -eq 'cmd') { Write-Pass "a spaced SHELL seeds default-shell" } else { Write-Fail "a spaced SHELL should run cmd, got '$cmdName'" }
    Stop-Case
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Stop-Case
    [Environment]::SetEnvironmentVariable('SHELL', $null)
    Start-Sleep -Milliseconds 500
    try { [System.IO.Directory]::Delete($TMP, $true) } catch {}
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
