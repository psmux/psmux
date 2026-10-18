# Issue #851: run-shell waits, and so does the next line of a sourced file;
# -d delays it.
#
# tmux's run-shell waits for its shell command unless -b is given, and the
# commands after it wait too (cmd-run-shell.c:123 and :186 to :188, tag 3.7c).
# -d delay waits that many seconds before starting the command, and with no
# command it only waits (:125 to :131 and :178 to :184). A delay that is not a
# number is "invalid delay time: <value>".
#
# psmux started a run-shell in a sourced file without waiting, whatever its
# flags, so the next line ran at once, and -d was not read on any route: it
# went to the shell as part of the command.
#
# Every expected value below is what tmux 3.7c gave for the same lines (with
# `sleep` where this suite uses Start-Sleep).
#
# The suite runs under -L in a PSMUX_DATA_DIR of its own and kills only its own
# namespace. Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rsw_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "rsw" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "rs"
# The binary a shell command calls back into, quoted for PowerShell.
$CALL = "& '$PSMUX' -L $NS"

Write-Host "`n=== Issue #851: run-shell waits, and -d delays it ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
# An option never set reads "invalid option: NAME", so anything but the value
# fails the check.
function Opt($name) { ((P show-options -gv $name) -join '').Trim() }

# Run psmux with these arguments: exit code, stdout, stderr and how long it
# took in milliseconds.
function Run([string[]]$argv) {
    $out = Join-Path $TMP "out.txt"; $err = Join-Path $TMP "err.txt"
    $list = @("-L", $NS) + @($argv | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } })
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $PSMUX -ArgumentList $list -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $out -RedirectStandardError $err
    $ms = $sw.ElapsedMilliseconds
    return @{
        Code = $p.ExitCode; Ms = $ms
        Out  = ((Get-Content $out -Raw -EA SilentlyContinue) + "").Trim()
        Err  = ((Get-Content $err -Raw -EA SilentlyContinue) + "").Trim()
    }
}

# Write these lines to a file and run the CLI `source-file` on it.
function Source([string[]]$lines) {
    $path = Join-Path $TMP "case.conf"
    $lines | Set-Content -Path $path -Encoding ASCII
    return Run @("source-file", $path)
}

function Reset-Session {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 500
}

try {
    # ── A sourced file ──
    Write-Head "A. the line after a run-shell waits for it"
    Reset-Session
    $r = Source @('run-shell "Start-Sleep -Milliseconds 1500"', 'set -g @after on')
    Check ($r.Code -eq 0) "exits 0 (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    Check ($r.Ms -ge 1400) "source-file returned after the shell ($($r.Ms) ms) (tmux)" "returned after $($r.Ms) ms"
    Check ((Opt '@after') -eq 'on') "the next line ran (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "B. a shell command that calls back runs before the next line"
    Reset-Session
    Source @("run-shell `"Start-Sleep -Milliseconds 800; $CALL set -g '@x' a`"", 'set -ga @x b') | Out-Null
    Check ((Opt '@x') -eq 'ab') "@x is ab (tmux)" "@x reads [$(Opt '@x')]"

    Write-Head "C. the command's output reaches the client"
    Reset-Session
    $r = Source @('run-shell "Write-Output hi"')
    Check ($r.Out -match '(^|\n)hi\s*$') "stdout is hi (tmux)" "stdout [$($r.Out)], stderr [$($r.Err)]"

    Write-Head "D. -d with no command only waits"
    Reset-Session
    $r = Source @('run-shell -d 1', 'set -g @after on')
    Check ($r.Ms -ge 900) "source-file returned after the delay ($($r.Ms) ms) (tmux)" "returned after $($r.Ms) ms"
    Check ((Opt '@after') -eq 'on') "the next line ran (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "E. -bd starts the command later and nothing waits"
    Reset-Session
    $r = Source @("run-shell -bd 1 `"$CALL set -g '@x' late`"", 'set -g @after on')
    Check ($r.Ms -lt 900) "source-file returned at once ($($r.Ms) ms) (tmux)" "returned after $($r.Ms) ms"
    Check (((Opt '@after') -eq 'on') -and ((Opt '@x') -ne 'late')) "the next line ran first (tmux)" "@after [$(Opt '@after')], @x [$(Opt '@x')]"
    Start-Sleep -Milliseconds 3500
    Check ((Opt '@x') -eq 'late') "the command ran after the delay (tmux)" "@x reads [$(Opt '@x')]"

    Write-Head "F. -b does not wait"
    Reset-Session
    $r = Source @('run-shell -b "Start-Sleep -Milliseconds 1500"', 'set -g @after on')
    Check ($r.Ms -lt 1200) "source-file returned at once ($($r.Ms) ms) (tmux)" "returned after $($r.Ms) ms"
    Check ((Opt '@after') -eq 'on') "the next line ran (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "G. a delay that is not a number"
    Reset-Session
    $r = Source @('run-shell -d soon "Write-Output hi"', 'set -g @after on')
    Check (($r.Code -eq 1) -and ($r.Err -match 'invalid delay time: soon')) "exit 1, invalid delay time: soon (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    Check ((Opt '@after') -eq 'on') "the rest of the file still ran (tmux)" "@after reads [$(Opt '@after')]"

    # ── The CLI ──
    Write-Head "H. the CLI"
    Reset-Session
    $r = Run @("run-shell", "-d", "1")
    Check (($r.Code -eq 0) -and ($r.Ms -ge 900)) "run-shell -d 1 waits and exits 0 ($($r.Ms) ms) (tmux)" "exit $($r.Code) after $($r.Ms) ms, stderr [$($r.Err)]"
    $r = Run @("run-shell", "-d", "soon", "Write-Output hi")
    Check (($r.Code -eq 1) -and ($r.Err -match 'invalid delay time: soon')) "an invalid delay is exit 1 (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    $r = Run @("run-shell", "-bd", "1", "$CALL set -g '@y' late")
    Check (($r.Code -eq 0) -and ($r.Ms -lt 900)) "run-shell -bd 1 returns at once ($($r.Ms) ms) (tmux)" "exit $($r.Code) after $($r.Ms) ms, stderr [$($r.Err)]"
    Start-Sleep -Milliseconds 3500
    Check ((Opt '@y') -eq 'late') "and the command ran after the delay (tmux)" "@y reads [$(Opt '@y')]"
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
