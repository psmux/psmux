# Issue #1066: a pane program's clipboard query, OSC 52 ; c ; ?, gets no
# reply, and get-clipboard does nothing.
#
# Clipboard read: OSC 52 ; selection ; ? from a pane, answered as the
# get-clipboard option says (input_osc_52_parse and input_osc_52_reply,
# input.c:3341-3411; options-table.c:440; all at tag 3.8). psmux kept the
# option's name only and gave no reply.
#
# Every expectation is what tmux 3.8 did, measured with the same bytes and
# options, except where a comment says otherwise. What the inbox conhost does
# with the query differs by build (PSMUX_PANE_RAW): Windows 11's (26200,
# 26300) drops it and answers a status report itself, so only those replies
# come; Windows 10's (19045) passes both on when it next draws, so psmux
# answers as through OpenConsole, which passes them on. Each case's bytes end
# with text so that the host draws.
#
# In [1] and [2] no client is attached, so `request` and `both` never read
# the system clipboard: tmux has no terminal to ask then and gives no reply,
# and so does psmux. [3] attaches one and reads it while another window holds
# it open (#428); psmux reads the Windows clipboard where tmux asks the
# terminal and waits for its answer (tty_clipboard_query, tty.c:3207).
#
# tests/reply_order_probe.cs runs in the pane with `raw`: it writes a case's
# bytes in one write and records the replies for 2 s.
#   1. the inbox conhost.
#   2. OpenConsole, when PSMUX_TEST_CONPTY_DIR or PSMUX_CONPTY_DIR_FOR_TEST
#      names a folder holding conpty.dll, skipped otherwise.
#   3. OpenConsole with a client attached (tests/pipe_client_cursor_colour_log.cs
#      plays its terminal) and the clipboard held by tests/clipboard_holder.cs.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server` and the client and holders this suite
# started, stopped by PID. [3] saves the user's clipboard and puts it back.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } elseif ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $env:PSMUX_CONPTY_DIR_FOR_TEST } else { '' }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_clip_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
}
$TermExe = Join-Path $TempHome 'pipe_client_cursor_colour_log.exe'
$holder = Join-Path $TempHome 'clipboard_holder.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'pipe_client_cursor_colour_log.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize -r:System.Windows.Forms.dll /out:$holder (Join-Path $PSScriptRoot 'clipboard_holder.cs') 2>&1 | Out-Null
}
. (Join-Path $PSScriptRoot 'clipboard_test_helpers.ps1')
if (-not (Test-Path $probe) -or -not (Test-Path $TermExe) -or -not (Test-Path $holder)) {
    Write-Fail "build tests\reply_order_probe.cs, pipe_client_cursor_colour_log.cs and clipboard_holder.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$A = '\a'; $ST = '\e\'
$Ask = "\e]52;c;?$ST"
function Reply($sel, $b64, $end) { "\e]52;$sel;$b64$end" }
$script:NS = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

# Type once the shell in $target shows its prompt: on a slow machine it can
# take longer to start than a fixed wait, and keys sent before may be lost.
function Wait-Prompt($target) {
    for ($k = 0; $k -lt 100 -and -not ((P capture-pane -p -t $target | Out-String) -match 'PS [^\r\n]*>'); $k++) {
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 500
}

# The reply file, or `(no reply file)` with what $target shows, so a probe
# that never ran or never finished can be told from one that got nothing.
function Read-Reply($out, $target) {
    if (Test-Path $out) { return (Get-Content $out -Raw).Trim() }
    Write-Info "no reply file; ${target} shows:"
    P capture-pane -p -t $target -S -20 | Where-Object { "$_".Trim() } | ForEach-Object { Write-Info "  | $_" }
    '(no reply file)'
}

# Run $c.q from a new window after $c.setup and check the reply. A reply given
# as @{ inbox = ...; win10 = ...; oc = ... } differs by route, `win10` being
# the inbox conhost before Windows 11; a plain value is all of them. The
# bytes end with a `.`.
function Case($route, $c) {
    $script:run++
    $window = "w$($script:run)"
    P new-window -t s -n $window | Out-Null
    Wait-Prompt "s:$window"
    if ($c.setup) { & $c.setup; Start-Sleep -Milliseconds 300 }
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:$window" "& '$probe' '$out' '$($c.q).' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 500
    $key = if ($route -eq 'OpenConsole') { 'oc' } elseif ($InboxForwardsQueries) { 'win10' } else { 'inbox' }
    $want = if ($c.reply -is [hashtable]) { $c.reply[$key] } else { $c.reply }
    $got = Read-Reply $out "s:$window"
    Check "${route}: $($c.n)" $got $want
}

# The inbox conhost before Windows 11 passes OSC 52 ; ? and DSR on to psmux
# (measured on 19045); the one on 26200 and 26300 drops the query.
$InboxForwardsQueries = [Environment]::OSVersion.Version.Build -lt 22000
$oc = { param($v) @{ inbox = ''; win10 = $v; oc = $v } }
$Hello = 'aGVsbG8='
$cases = @(
    @{ n = 'no buffer'; q = $Ask; reply = '' }
    # Measured: paste_get_top skips a named buffer.
    @{ n = 'a named buffer only'; setup = { P set-buffer -b foo named | Out-Null }; q = $Ask; reply = '' }
    @{ n = 'the newest automatic buffer'; setup = { P set-buffer hello | Out-Null }; q = $Ask; reply = (& $oc (Reply 'c' $Hello $ST)) }
    # Measured on tmux 3.8: with set-clipboard external, its default, the
    # query gets no reply (input_osc_52_parse returns unless it is `on`,
    # input.c:3379). Nothing printed in a pane reads the buffers back unless
    # the user opts in.
    @{ n = 'external, the default: no reply even with a buffer'; setup = { P set -s set-clipboard external | Out-Null }; q = $Ask; reply = '' }
    @{ n = 'asked with BEL'; setup = { P set -s set-clipboard on | Out-Null }; q = "\e]52;c;?$A"; reply = (& $oc (Reply 'c' $Hello $A)) }
    @{ n = 'no selection'; q = "\e]52;;?$ST"; reply = (& $oc (Reply '' $Hello $ST)) }
    @{ n = 'the first selection tmux knows'; q = "\e]52;xp;?$ST"; reply = (& $oc (Reply 'p' $Hello $ST)) }
    @{ n = 'a selection tmux does not know'; q = "\e]52;x;?$ST"; reply = (& $oc (Reply '' $Hello $ST)) }
    # tmux reads OSC 52, a write or a query, only with set-clipboard on
    # (input.c:3379): on `external` neither makes a buffer nor gets a reply.
    @{ n = 'external: a write and a query, no reply'; setup = { P set -s set-clipboard external | Out-Null }; q = "\e]52;c;d3JpdHRlbg==$ST$Ask"; reply = '' }
    # Measured: on `on` a write earlier in the same output is read back.
    @{ n = 'on: a write before the query is read back'; setup = { P set -s set-clipboard on | Out-Null }; q = "\e]52;c;d3JpdHRlbg==$ST$Ask"; reply = (& $oc (Reply 'c' 'd3JpdHRlbg==' $ST)) }
    @{ n = 'set-clipboard off'; setup = { P set -s set-clipboard off | Out-Null }; q = $Ask; reply = '' }
    @{ n = 'get-clipboard off'; setup = { P set -s set-clipboard on | Out-Null; P set -s get-clipboard off | Out-Null }; q = $Ask; reply = '' }
    # Measured: with no client tmux has no terminal to ask.
    @{ n = 'request with no client attached'; setup = { P set -s get-clipboard request | Out-Null }; q = $Ask; reply = '' }
    @{ n = 'both with no client attached'; setup = { P set -s get-clipboard both | Out-Null }; q = $Ask; reply = '' }
    @{ n = 'buffer again'; setup = { P set -s get-clipboard buffer | Out-Null }; q = $Ask; reply = (& $oc (Reply 'c' 'd3JpdHRlbg==' $ST)) }
    # The replies go in the order asked, with the ones around it (#930).
    @{ n = 'between two status reports'; q = "\e[5n$Ask\e[5n"; reply = @{ inbox = '\e[0n\e[0n'; win10 = '\e[0n' + (Reply 'c' 'd3JpdHRlbg==' $ST) + '\e[0n'; oc = '\e[0n' + (Reply 'c' 'd3JpdHRlbg==' $ST) + '\e[0n' } }
)

function Run-Cases($route) {
    $script:NS = "cr_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:run = 0
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500

    Write-Host "  -- the option" -ForegroundColor DarkCyan
    Check "${route}: get-clipboard's default" "$(P show -sv get-clipboard)" 'buffer'
    Check "${route}: set-clipboard's default" "$(P show -sv set-clipboard)" 'external'
    # Measured: tmux says `unknown value: bad` and keeps the old value.
    Check "${route}: a bad value is refused" ("$(P set -s get-clipboard bad)" -like '*unknown value: bad') $true
    Check "${route}: and the value is kept" "$(P show -sv get-clipboard)" 'buffer'

    Write-Host "  -- the query" -ForegroundColor DarkCyan
    # The cases opt in; the ones that say `external` go back to the default.
    P set -s set-clipboard on | Out-Null
    foreach ($c in $cases) { Case $route $c }

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

# `request` with a client attached, the clipboard held open by another window
# for $c.hold ms: the query waits for it, up to 1 s as a paste does (#428),
# and the status report after it keeps its place.
$script:Term = $null
$script:holders = @()
function Run-Busy {
    $script:NS = "cb_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:run = 0
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $log = Join-Path $TempHome "outer_$($script:NS).log"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    for ($i = 0; $i -lt 50 -and "$(P list-clients)" -notmatch 'attached'; $i++) { Start-Sleep -Milliseconds 200 }
    Check "busy clipboard: client attached" ("$(P list-clients)" -match 'attached') $true
    P set -s get-clipboard request | Out-Null
    P set -s set-clipboard on | Out-Null
    $saved = Get-ClipboardSettled
    try {
        if (-not (Set-ClipboardVerified 'B12BUSY')) {
            Write-Info "skipped: the desktop kept the clipboard from this process"
            return
        }
        $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('B12BUSY'))
        foreach ($c in @(
                # The default, set-clipboard external, never reads the
                # Windows clipboard for a pane, even with get-clipboard
                # request and a client attached, as tmux never asks there.
                @{ hold = 0; clip = 'external'; n = 'external, the default: the Windows clipboard is not read'; reply = '\e[0n' }
                @{ hold = 0; clip = 'on'; n = 'a free clipboard'; reply = (Reply 'c' $b64 $ST) + '\e[0n' }
                @{ hold = 300; n = 'held 300 ms, answered once it is let go'; reply = (Reply 'c' $b64 $ST) + '\e[0n' }
                # psmux: tmux waits 5 s for its terminal; psmux waits 1 s, as
                # for a paste, then answers the rest of the batch.
                @{ hold = 1500; n = 'held 1500 ms, past the wait, so no clipboard reply'; reply = '\e[0n' })) {
            $script:run++
            $window = "w$($script:run)"
            if ($c.clip) { P set -s set-clipboard $c.clip | Out-Null }
            P new-window -t s -n $window | Out-Null
            Wait-Prompt "s:$window"
            if ($c.hold) {
                $ready = Join-Path $TempHome "held_$($script:run)"
                $script:holders += Start-Process $holder -ArgumentList $c.hold, $ready -PassThru -WindowStyle Hidden
                for ($k = 0; $k -lt 100 -and -not (Test-Path $ready); $k++) { Start-Sleep -Milliseconds 20 }
            }
            $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
            P send-keys -t "s:$window" "& '$probe' '$out' '$Ask\e[5n.' raw" Enter | Out-Null
            for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
            Start-Sleep -Milliseconds 500
            $got = Read-Reply $out "s:$window"
            Check "busy clipboard: $($c.n)" $got $c.reply
            foreach ($h in $script:holders) { if (-not $h.HasExited) { $h.WaitForExit(6000) | Out-Null } }
        }
    } finally {
        if ($saved) { Set-ClipboardVerified $saved | Out-Null }
        if ($script:Term -and -not $script:Term.HasExited) { & taskkill /PID $script:Term.Id /T /F 2>&1 | Out-Null }
        $script:Term = $null
        P kill-server | Out-Null
        Start-Sleep -Milliseconds 500
    }
}

Write-Host "`n=== Clipboard read (OSC 52 ; ?) ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'

        Write-Host "`n[3] a clipboard another window holds open (OpenConsole)" -ForegroundColor Yellow
        Run-Busy
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:Term -and -not $script:Term.HasExited) { & taskkill /PID $script:Term.Id /T /F 2>&1 | Out-Null }
    foreach ($h in $script:holders) { if (-not $h.HasExited) { & taskkill /PID $h.Id /F 2>&1 | Out-Null } }
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
