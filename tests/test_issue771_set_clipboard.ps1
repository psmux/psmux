# Issue #771: set-clipboard, two differences from tmux.
#
# 1. the default is `on` where tmux's is `external`
# 2. with `external`, an application's OSC 52 still creates a paste buffer,
#    where tmux takes one only on `on` (options-table.c: "whether to allow
#    applications to create paste buffers with an escape sequence ('on'
#    only)", and input.c returns before decoding unless the value is 2)
#
# GROUND TRUTH: a detached session of its own under -L, one OSC 52 written
# from inside the pane, and `list-buffers` read over the CLI. No attached
# client is needed: the payload is ingested server side.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

# A config of its own, or the machine's own `set-clipboard` line answers
# section 1 instead of the default (discussion #748).
. "$PSScriptRoot\isolated_config.ps1"

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$NS  = "osc-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_osc_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
function P { & $PSMUX -L $NS @args 2>&1 }

# A script the pane runs to write one OSC 52 carrying OSC52MARK.
$emit = Join-Path $TMP "emit.ps1"
@(
    '$b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("OSC52MARK"))',
    '$e = [char]27',
    '[Console]::Out.Write("$e]52;c;$b64$e\")',
    '[Console]::Out.Flush()'
) | Set-Content -Path $emit -Encoding ASCII

P kill-server | Out-Null
Start-Sleep -Milliseconds 500
P new-session -d -s one | Out-Null
Start-Sleep -Milliseconds 2000

function Marks {
    @(P list-buffers | Where-Object { $_ -match 'OSC52MARK' }).Count
}
function Emit {
    # Clear what is there, write one OSC 52, give the server a moment to drain.
    for ($i = 0; $i -lt 12; $i++) { P delete-buffer | Out-Null }
    P send-keys -t one "& '$emit'" Enter | Out-Null
    Start-Sleep -Milliseconds 2500
}

# ── 1. the default ──

Write-Head "1. the default is external, as tmux"
$opt = ((P show-options -s | Select-String 'set-clipboard') -join '').Trim()
Write-Info "show-options: [$opt]"
Check ($opt -match 'set-clipboard\s+external') "the default is external" "it reads [$opt]"

# ── 2. off does neither ──
#
# First, while nothing has been staged yet: the forward slot is one shot and
# only an attached client consumes it, so a payload staged by a later section
# would still be sitting there when this one looked.

Write-Head "2. off neither buffers nor forwards"
P set-option -s set-clipboard off | Out-Null
Emit
$n = Marks
Check ($n -eq 0) "off made no buffer" "it made $n"
$fwd = ((P dump-state) -join '') -match 'clipboard_osc52'
Check (-not $fwd) "and nothing is staged to forward" "a payload was staged"

# ── 3. external forwards and does not make a buffer ──

Write-Head "3. external does not let the application make a buffer"
P set-option -s set-clipboard external | Out-Null
Emit
$n = Marks
Write-Info "buffers carrying the mark: $n"
Check ($n -eq 0) "external made no buffer" "it made $n"
$fwd = ((P dump-state) -join '') -match 'clipboard_osc52'
Check $fwd "the payload is still staged for the outer terminal" "nothing was staged to forward"

# ── 4. on still makes one ──

Write-Head "4. on still makes a buffer"
P set-option -s set-clipboard on | Out-Null
Emit
$n = Marks
Write-Info "buffers carrying the mark: $n"
Check ($n -eq 1) "on made the buffer" "it made $n"

# ── Cleanup ──

P kill-server | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
Remove-PsmuxIsolatedConfig

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
