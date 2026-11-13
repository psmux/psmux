# Issue #972 follow up: when Claude Code started in psmux on Windows 10, the
# text `tmux;` appeared over the prompt placeholder.
#
# Under TMUX, Claude Code wraps its OSC 11 background colour query in a tmux
# passthrough: `ESC P tmux; ESC ESC ] 11;? BEL ESC \`. The inbox conhost of
# Windows 10 (19045) has no DCS parser. It prints `tmux;` into its buffer at
# the cursor and forwards the inner OSC with both escapes, then paints the five
# cells. psmux now keeps the cells the application drew there and only moves
# the cursor past them, as conhost counted them. tmux drops such a DCS while
# allow-passthrough is off, and psmux does the same when the DCS arrives whole
# (Windows 11, OpenConsole), so on those hosts this test passes on any build.
#
# The pane program draws a prompt, parks the cursor in it, sends the wrapped
# query the way Claude Code does, then writes a marker into the prompt row, and
# later prints `tmux;` as real text on another row, which must stay. The unit
# tests in tests-rs/test_issue972_conhost_dcs_body.rs check the cursor count.
#
# Needs node. Runs under a private -L namespace and PSMUX_DATA_DIR; cleans up
# with `psmux -L <ns> kill-server` only. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow }

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Skip "node is not installed"
    exit 0
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i972dcs_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$savedEnv = @{}
foreach ($n in 'PSMUX_DATA_DIR', 'PSMUX_CONPTY_DIR', 'PSMUX_NO_WARM') { $savedEnv[$n] = [Environment]::GetEnvironmentVariable($n) }
$env:PSMUX_DATA_DIR = $work
$env:PSMUX_NO_WARM = '1'
Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue
Remove-Item Env:TMUX -ErrorAction SilentlyContinue
Remove-Item Env:TMUX_PANE -ErrorAction SilentlyContinue

$child = Join-Path $work 'dcs_child.mjs'
@'
const E = '\x1b';
const out = (s) => process.stdout.write(s);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
// What Claude Code writes for its OSC 11 query when TMUX is set.
const wrapped = (s) => `${E}Ptmux;${s.replaceAll(E, E + E)}${E}\\`;
out(`${E}[2J${E}[H`);
out(`${E}[2;1H❯ Try "refactor"`);
out(`${E}[2;3H`);
// Let the console host paint its first frame: it forwards the inner OSC
// only once the frame with the prompt is out, as with Claude Code.
await wait(1200);
out(wrapped(`${E}]11;?\x07`));
await wait(600);
out(`${E}[2;12HX`);
await wait(400);
out(`${E}[4;1Htmux; is text`);
await wait(400);
out(`${E}[6;1HI972DCS-DONE`);
setInterval(() => {}, 1000);
'@ | Set-Content -Path $child -Encoding UTF8

function Run-Child([string]$conptyDir) {
    $ns = 'i972dcs_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    if ($conptyDir) { $env:PSMUX_CONPTY_DIR = $conptyDir } else { Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }
    try {
        & $PSMUX -L $ns new-session -d -s t -x 60 -y 10 "node `"$child`""
        $lines = $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 30) {
            Start-Sleep -Milliseconds 400
            $lines = & $PSMUX -L $ns capture-pane -p -t t 2>$null
            if (($lines -join "`n") -match 'I972DCS-DONE') { break }
        }
        return , $lines
    } finally {
        & $PSMUX -L $ns kill-server 2>$null | Out-Null
        Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue
    }
}

function Check([string]$label, $lines) {
    if (-not (($lines -join "`n") -match 'I972DCS-DONE')) {
        Write-Fail "${label}: the pane program never finished; capture:`n$($lines -join "`n")"
        return
    }
    $prompt = [string][char]0x276F + [char]0x00A0 + 'Try "refactor"'
    $want = $prompt.Substring(0, 11) + 'X' + $prompt.Substring(12)
    $got = $lines[1].TrimEnd()
    if ($got -eq $want) { Write-Pass "${label}: prompt row has no tmux; ($got)" }
    else { Write-Fail "${label}: prompt row expected '$want', got '$got'" }
    $txt = $lines[3].TrimEnd()
    if ($txt -eq 'tmux; is text') { Write-Pass "${label}: tmux; written as text is drawn" }
    else { Write-Fail "${label}: text row expected 'tmux; is text', got '$txt'" }
}

Write-Host "`n=== Issue #972: tmux; from a DCS the inbox conhost cannot parse ===" -ForegroundColor Cyan
try {
    Check 'inbox' (Run-Child '')
    $occ = if ($env:PSMUX_TEST_OPENCONSOLE_DIR) { $env:PSMUX_TEST_OPENCONSOLE_DIR } else { Join-Path $env:TEMP 'occ-conpty\nugetstable' }
    if (Test-Path (Join-Path $occ 'conpty.dll')) {
        Check 'OpenConsole' (Run-Child $occ)
    } else {
        Write-Skip "no NuGet OpenConsole at $occ"
    }
} finally {
    foreach ($n in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($n, $savedEnv[$n]) }
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
