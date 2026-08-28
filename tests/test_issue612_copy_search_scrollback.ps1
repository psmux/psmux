# test_issue612_copy_search_scrollback.ps1
#
# Issue #612 (kirill146): "All of `?`, `/`, `Ctrl+s`, `Ctrl+r` can't see past
# the on-screen area of the buffer."
#
# Reproduced on an 80x24 pane holding LINE_1 .. LINE_400. Before the fix,
# typing `?` then LINE_120 left the cursor exactly where it was and cleared
# #{search_match}:
#
#   after-visible   cy=6  cline=[LINE_385] scroll=0 match=[LINE_385] present=1
#   after-history   cy=6  cline=[LINE_385] scroll=0 match=[]         present=1
#
# Root cause: copy_mode::search_copy_mode scanned only 0..p.last_rows, the rows
# the vt100 screen currently frames, and stored hits as VISIBLE row indexes.
# The scrollback history was never read and nothing moved the viewport.
#
# tmux 3.4 walks the whole grid: window_copy_search_jump (window-copy.c:4470)
# iterates absolute lines 0 .. gd->hsize + gd->sy - 1 and then calls
# window_copy_scroll_to, which parks an off screen match a quarter of a screen
# up from the bottom. Measured in WSL on the same 400 line / 80x24 geometry,
# real tmux answers `search-backward LINE_5` with copy_cursor_line=LINE_59 and
# scroll_position=338; psmux now answers with the same pair.
#
# Layer 1 (CLI + display-message) drives the server command path: the
#         `send-keys -X search-backward <term>` scripting surface and the
#         interactive `?` / `/` prompt reached by literal send-keys.
# Layer 2 (MANDATORY attached Win32 TUI) launches a real attached client, then
#         injects real WriteConsoleInput KEY_EVENT records so the search runs
#         through the TUI input path a user actually presses keys into.

param([string]$PsmuxPath = "")

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$env:PSMUX_NO_WARM = "1"

$PSMUX = if ($PsmuxPath) { $PsmuxPath }
         elseif ($env:PSMUX_EXE) { $env:PSMUX_EXE }
         else { (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $PSMUX -or -not (Test-Path $PSMUX)) { Write-Host "[FAIL] psmux not found (set PSMUX_EXE)"; exit 1 }
$PSMUX = (Resolve-Path $PSMUX).Path

$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }

$ROWS = 24
$COLS = 80
$pass = 0
$fail = 0
$skip = 0

function Write-Pass($m) { Write-Host "[PASS] $m" -ForegroundColor Green; $script:pass++ }
function Write-Fail($m) { Write-Host "[FAIL] $m" -ForegroundColor Red; $script:fail++ }
function Write-Skip($m) { Write-Host "[SKIP] $m" -ForegroundColor Yellow; $script:skip++ }

# One display-message round trip that returns every copy mode variable at once.
function Get-CopyState([string]$Target) {
    $fmt = 'cx=#{copy_cursor_x}|cy=#{copy_cursor_y}|line=#{copy_cursor_line}|scroll=#{scroll_position}|match=#{search_match}|present=#{search_present}|hist=#{history_size}|inmode=#{pane_in_mode}'
    $raw = (& $PSMUX display-message -t $Target -p $fmt 2>&1 | Out-String).Trim()
    $o = [ordered]@{ raw = $raw }
    foreach ($part in ($raw -split '\|')) {
        $kv = $part -split '=', 2
        if ($kv.Count -eq 2) { $o[$kv[0]] = $kv[1] }
    }
    [pscustomobject]$o
}

function Get-TopVisible([string]$Target) {
    $lines = (& $PSMUX capture-pane -t $Target -p 2>&1 | Out-String) -split "`r?`n"
    if ($lines.Count -gt 0) { return $lines[0].TrimEnd() }
    return ""
}

function Fill-Pane([string]$Target) {
    & $PSMUX send-keys -t $Target 'for ($i=1;$i -le 400;$i++){"LINE_$i"}' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 4
}

# Drive the interactive prompt exactly as a user does: opener key, then the
# term one literal character at a time, then Enter.
function Invoke-PromptSearch([string]$Target, [string]$Opener, [string]$Term) {
    & $PSMUX send-keys -t $Target $Opener 2>&1 | Out-Null
    Start-Sleep -Milliseconds 250
    foreach ($ch in $Term.ToCharArray()) {
        & $PSMUX send-keys -t $Target -l "$ch" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Milliseconds 150
    & $PSMUX send-keys -t $Target Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
}

Write-Host "`n=== ISSUE #612: copy mode search must walk the whole scrollback ===" -ForegroundColor Cyan
Write-Host "psmux    : $PSMUX"
Write-Host "data root: $psmuxDir"

# ══════════════════════════════════════════════════════════════════════════
# Layer 1: CLI command path
# ══════════════════════════════════════════════════════════════════════════
Write-Host "`n--- Layer 1: CLI send-keys -X and the literal '?' / '/' prompt ---" -ForegroundColor Cyan

$S = "i612cli"
& $PSMUX kill-session -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
& $PSMUX new-session -d -s $S -x $COLS -y $ROWS 2>&1 | Out-Null
Start-Sleep -Milliseconds 1200
$T = "${S}:0"
Fill-Pane $T

& $PSMUX copy-mode -t $T 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$st = Get-CopyState $T
if ($st.inmode -ne "1") {
    Write-Skip "copy-mode did not take, nothing below can be proven"
} else {
    Write-Pass "copy-mode entered, history_size=$($st.hist) scroll=$($st.scroll)"
    $topBefore = Get-TopVisible $T
    Write-Host "      top visible line before any search: [$topBefore]"

    # ── 1a. send-keys -X search-backward <term>, deep in the scrollback ──
    & $PSMUX send-keys -t $T -X search-backward LINE_120 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $a = Get-CopyState $T
    if ($a.line -eq "LINE_120") {
        Write-Pass "-X search-backward LINE_120 landed ON the match (cy=$($a.cy) scroll=$($a.scroll))"
    } else {
        Write-Fail "-X search-backward LINE_120 expected copy_cursor_line=LINE_120, got [$($a.line)] scroll=$($a.scroll)"
    }
    if ([int]$a.scroll -gt 0) {
        Write-Pass "the viewport scrolled back into history, scroll_position=$($a.scroll)"
    } else {
        Write-Fail "the viewport never left the live bottom, scroll_position=$($a.scroll)"
    }
    if ($a.match -eq "LINE_120") {
        Write-Pass "#{search_match} reports the term that was found"
    } else {
        Write-Fail "#{search_match} expected LINE_120, got [$($a.match)]"
    }
    $topAfter = Get-TopVisible $T
    if ($topAfter -ne $topBefore -and $topAfter -match '^LINE_\d+$') {
        Write-Pass "capture-pane shows the scrolled region, top line went [$topBefore] -> [$topAfter]"
    } else {
        Write-Fail "capture-pane top line did not move into history, [$topBefore] -> [$topAfter]"
    }

    # ── 1b. tmux parity on the exact geometry measured in WSL ──
    & $PSMUX send-keys -t $T -X history-bottom 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX send-keys -t $T -X search-backward LINE_5 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $b = Get-CopyState $T
    if ($b.line -eq "LINE_59" -and $b.scroll -eq "338") {
        Write-Pass "tmux 3.4 parity: search-backward LINE_5 gives LINE_59 at scroll_position 338"
    } else {
        Write-Fail "tmux parity: expected LINE_59 / scroll 338, got [$($b.line)] / scroll $($b.scroll)"
    }

    # ── 1c. n / N repeat, still inside history ──
    & $PSMUX send-keys -t $T -X search-again 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $c = Get-CopyState $T
    if ($c.line -eq "LINE_58") {
        Write-Pass "search-again (n) walked on upward to LINE_58"
    } else {
        Write-Fail "search-again expected LINE_58, got [$($c.line)]"
    }
    & $PSMUX send-keys -t $T -X search-reverse 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $d = Get-CopyState $T
    if ($d.line -eq "LINE_59") {
        Write-Pass "search-reverse (N) stepped back to LINE_59"
    } else {
        Write-Fail "search-reverse expected LINE_59, got [$($d.line)]"
    }

    # ── 1d. forward search reaches a match far BELOW the viewport ──
    & $PSMUX send-keys -t $T -X history-top 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    $top = Get-CopyState $T
    & $PSMUX send-keys -t $T -X search-forward LINE_390 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $e = Get-CopyState $T
    if ($e.line -eq "LINE_390" -and [int]$e.scroll -lt [int]$top.scroll) {
        Write-Pass "-X search-forward LINE_390 came down from the top of history, scroll $($top.scroll) -> $($e.scroll)"
    } else {
        Write-Fail "-X search-forward LINE_390 expected LINE_390 below scroll $($top.scroll), got [$($e.line)] / scroll $($e.scroll)"
    }

    # ── 1e. the interactive `?` prompt, the exact key the reporter pressed ──
    & $PSMUX send-keys -t $T -X history-bottom 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    Invoke-PromptSearch $T "?" "LINE_120"
    $f = Get-CopyState $T
    if ($f.line -eq "LINE_120" -and [int]$f.scroll -gt 0) {
        Write-Pass "the '?' prompt reached LINE_120 in history (cy=$($f.cy) scroll=$($f.scroll))"
    } else {
        Write-Fail "the '?' prompt expected LINE_120 with a nonzero scroll, got [$($f.line)] / scroll $($f.scroll)"
    }
    if ($f.inmode -eq "1") {
        Write-Pass "the pane is still in copy mode after committing the prompt search"
    } else {
        Write-Fail "the pane left copy mode after the prompt search, pane_in_mode=$($f.inmode)"
    }

    # ── 1f. the interactive `/` prompt from the top of history ──
    & $PSMUX send-keys -t $T -X history-top 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    $top2 = Get-CopyState $T
    Invoke-PromptSearch $T "/" "LINE_390"
    $g = Get-CopyState $T
    if ($g.line -eq "LINE_390" -and [int]$g.scroll -lt [int]$top2.scroll) {
        Write-Pass "the '/' prompt reached LINE_390 far below the viewport, scroll $($top2.scroll) -> $($g.scroll)"
    } else {
        Write-Fail "the '/' prompt expected LINE_390, got [$($g.line)] / scroll $($g.scroll)"
    }

    # ── 1g. a term that exists nowhere must not move anything ──
    $h0 = Get-CopyState $T
    Invoke-PromptSearch $T "?" "NOT_IN_BUFFER_612"
    $h = Get-CopyState $T
    if ($h.line -eq $h0.line -and $h.scroll -eq $h0.scroll) {
        Write-Pass "a term that is nowhere leaves the cursor and the viewport alone"
    } else {
        Write-Fail "a failed search moved something, [$($h0.line)]/$($h0.scroll) -> [$($h.line)]/$($h.scroll)"
    }

    # ── 1h. emacs Ctrl+r / Ctrl+s open the same prompt and reach history ──
    & $PSMUX send-keys -t $T -X history-bottom 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX set-option -t $S -g mode-keys emacs 2>&1 | Out-Null
    Invoke-PromptSearch $T "C-r" "LINE_120"
    $i = Get-CopyState $T
    if ($i.line -eq "LINE_120" -and [int]$i.scroll -gt 0) {
        Write-Pass "Ctrl+r reached LINE_120 in history (cy=$($i.cy) scroll=$($i.scroll))"
    } else {
        Write-Fail "Ctrl+r expected LINE_120 with a nonzero scroll, got [$($i.line)] / scroll $($i.scroll)"
    }
    & $PSMUX send-keys -t $T -X history-top 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    $top3 = Get-CopyState $T
    Invoke-PromptSearch $T "C-s" "LINE_390"
    $j = Get-CopyState $T
    if ($j.line -eq "LINE_390" -and [int]$j.scroll -lt [int]$top3.scroll) {
        Write-Pass "Ctrl+s reached LINE_390 far below the viewport, scroll $($top3.scroll) -> $($j.scroll)"
    } else {
        Write-Fail "Ctrl+s expected LINE_390, got [$($j.line)] / scroll $($j.scroll)"
    }
}

& $PSMUX kill-session -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 400

# ══════════════════════════════════════════════════════════════════════════
# Layer 2 (MANDATORY): attached Win32 TUI, real WriteConsoleInput keystrokes
# ══════════════════════════════════════════════════════════════════════════
Write-Host "`n--- Layer 2: attached TUI + WriteConsoleInput injection ---" -ForegroundColor Cyan

$injector = "$env:TEMP\psmux_injector_612.exe"
if (-not (Test-Path $injector)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) {
        $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
    }
    & $csc /nologo /optimize /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
}

if (-not (Test-Path $injector)) {
    Write-Skip "could not compile injector.cs, the attached TUI layer proves nothing"
} else {
    # The suite itself may run inside a psmux pane. An attached client refuses to
    # start there, so launch through a .cmd that scrubs the nesting variables.
    $ST = "i612tui"
    $launchCmd = "$env:TEMP\psmux_612_launch.cmd"
    $dataLine = if ($env:PSMUX_DATA_DIR) { "set PSMUX_DATA_DIR=$env:PSMUX_DATA_DIR" } else { "rem default data root" }
    @"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
$dataLine
"$PSMUX" new-session -s $ST -x $COLS -y $ROWS
"@ | Set-Content -Path $launchCmd -Encoding ASCII

    & $PSMUX kill-session -t $ST 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $null = Start-Process -FilePath $launchCmd -PassThru
    Start-Sleep -Seconds 7

    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s $ST" }

    if (-not $cli) {
        Write-Skip "no attached client came up, the injection layer proves nothing"
    } else {
        $clientPid = $cli.ProcessId
        Write-Host "      attached client pid=$clientPid"
        $TT = "${ST}:0"
        Fill-Pane $TT

        # prefix (C-b) then '[' enters copy mode, exactly as a user does it.
        & $injector $clientPid "^b" | Out-Null
        Start-Sleep -Milliseconds 400
        & $injector $clientPid "[" | Out-Null
        Start-Sleep -Milliseconds 900

        $t0 = Get-CopyState $TT
        if ($t0.inmode -ne "1") {
            Write-Skip "injected prefix+[ never reached the client, no keys were delivered"
        } else {
            Write-Pass "injected prefix+[ entered copy mode through the real TUI input path"

            # Real keystrokes: '?' then the term then Enter. LINE_12 is chosen
            # because it also matches LINE_120 .. LINE_129, so the 'n' repeat
            # below has somewhere to go.
            & $injector $clientPid "?{SLEEP:300}LINE_12{SLEEP:300}{ENTER}" | Out-Null
            Start-Sleep -Milliseconds 1200
            $t1 = Get-CopyState $TT
            if ($t1.line -eq "LINE_129" -and [int]$t1.scroll -gt 0) {
                Write-Pass "TUI: a real '?' LINE_12 Enter reached history and stopped at the nearest match LINE_129 (cy=$($t1.cy) scroll=$($t1.scroll))"
            } else {
                Write-Fail "TUI: the real '?' search expected LINE_129 with a nonzero scroll, got [$($t1.line)] / scroll $($t1.scroll)"
            }
            $tuiTop = Get-TopVisible $TT
            $tuiRows = [int]((& $PSMUX display-message -t $TT -p '#{pane_height}' 2>&1 | Out-String).Trim())
            if ($tuiTop -match '^LINE_(\d+)$' -and $t1.line -match '^LINE_(\d+)$') {
                $topN = [int]($tuiTop -replace 'LINE_', '')
                $hitN = [int]($t1.line -replace 'LINE_', '')
                if ($topN -le $hitN -and ($hitN - $topN) -lt $tuiRows) {
                    Write-Pass "TUI: capture-pane frames the match, top=[$tuiTop] match=[$($t1.line)] rows=$tuiRows"
                } else {
                    Write-Fail "TUI: capture-pane top [$tuiTop] does not frame the match [$($t1.line)] on $tuiRows rows"
                }
            } else {
                Write-Fail "TUI: expected a numbered top line and a numbered match, got top=[$tuiTop] match=[$($t1.line)]"
            }

            # A real 'n' repeats the search in the same direction.
            & $injector $clientPid "n" | Out-Null
            Start-Sleep -Milliseconds 800
            $t2 = Get-CopyState $TT
            if ($t2.line -eq "LINE_128") {
                Write-Pass "TUI: a real 'n' repeated the search upward, [$($t1.line)] -> [$($t2.line)]"
            } else {
                Write-Fail "TUI: a real 'n' expected LINE_128, got [$($t2.line)]"
            }
        }

        & $PSMUX kill-session -t $ST 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        Stop-Process -Id $clientPid -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "`n=== RESULT: $pass passed, $fail failed, $skip skipped ===" -ForegroundColor Cyan
if ($fail -gt 0) { exit 1 }
exit 0
