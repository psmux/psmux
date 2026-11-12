# Issue #88 verification — does `capture-pane -S` reliably retrieve
# scrollback that the user produced inside a psmux pane?
#
# Three distinct tests, each isolating one variable:
#
#   T1. Plain stdout (no TUI): 200 lines via PowerShell pipeline.
#       Baseline — if this fails, scrollback is broken end-to-end.
#
#   T2. TUI app using the alternate screen.  Output produced while
#       the alt screen is active should NOT land in main-grid
#       scrollback (this is correct vt100/tmux semantics).  Output
#       printed BEFORE entering alt screen, or AFTER exiting, MUST
#       still be in scrollback.  Tests whether we corrupt main
#       scrollback during alt-screen excursions.
#
#   T3. Codex itself, exact CXwudi scenario.  Run `codex exec`
#       (non-interactive — prints to stdout, no alt screen) and
#       ask it to emit 200 numbered lines.  Then capture-pane -S
#       must return all 200.  This is the literal repro from the
#       most recent issue comment.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

function Wait-Prompt {
    param([string]$Target, [int]$TimeoutMs = 15000, [string]$Pattern = "PS [A-Z]:\\")
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $cap = & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
            if ($cap -match $Pattern) { return $true }
        } catch {}
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Wait-Output {
    param([string]$Target, [string]$Marker, [int]$TimeoutMs = 60000)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $cap = & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
        if ($cap -match $Marker) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Reset-Server {
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Remove-Item "$psmuxDir\*.port","$psmuxDir\*.key","$psmuxDir\*.sess" -Force -EA SilentlyContinue
}

# ── T1: PLAIN STDOUT (200 lines) ───────────────────────────────────
Write-Host "`n=== T1: plain 200-line stdout, capture-pane -S -1000 ===" -ForegroundColor Cyan
Reset-Server
$SESSION = "iss88_plain"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4

if (-not (Wait-Prompt -Target $SESSION)) {
    Write-Fail "T1: shell not ready"
} else {
    Write-Pass "T1: shell ready"
    # Emit 200 numbered lines through PowerShell.
    & $PSMUX send-keys -t $SESSION '1..200 | ForEach-Object { "plain $_" }' Enter 2>&1 | Out-Null
    if (Wait-Output -Target $SESSION -Marker "plain 199" -TimeoutMs 30000) {
        Start-Sleep -Seconds 1
        # Capture deep scrollback (-S -1000 = 1000 rows back from top of view).
        $deep = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
        $matches = [regex]::Matches($deep, '(?m)^plain (\d+)\b')
        $count = $matches.Count
        $nums = $matches | ForEach-Object { [int]$_.Groups[1].Value }
        if ($count -gt 0) {
            $min = ($nums | Measure-Object -Minimum).Minimum
            $max = ($nums | Measure-Object -Maximum).Maximum
            Write-Info "T1: captured $count lines, range [$min..$max]"
        } else {
            Write-Info "T1: captured 0 'plain N' lines"
        }
        if ($count -ge 195) {
            Write-Pass "T1: capture-pane -S -1000 returns all 200 lines (got $count)"
        } else {
            Write-Fail "T1: capture-pane -S -1000 missed lines (got $count of 200)"
        }
    } else {
        Write-Fail "T1: 200 lines did not appear in pane"
    }
}
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

# ── T2: TUI APP (alt screen) ────────────────────────────────────────
# Use `more` (built-in pager): it does NOT switch to alt-screen so
# its output should land in scrollback normally.  Then use `vim`
# style: the alt-screen behaviour of TUI apps is well-known and
# we want a predictable test.  Skip if vim not available.
Write-Host "`n=== T2: pre/post-TUI scrollback survives alt-screen excursion ===" -ForegroundColor Cyan
Reset-Server
$SESSION = "iss88_tui"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (-not (Wait-Prompt -Target $SESSION)) {
    Write-Fail "T2: shell not ready"
} else {
    Write-Pass "T2: shell ready"
    # Step 1: emit 50 lines BEFORE any alt-screen excursion.
    & $PSMUX send-keys -t $SESSION '1..50 | ForEach-Object { "pre $_" }' Enter 2>&1 | Out-Null
    if (Wait-Output -Target $SESSION -Marker "pre 49" -TimeoutMs 30000) {
        Start-Sleep -Seconds 1
        # Step 2: emit raw alt-screen enter+exit via printf-style escape.
        # ESC[?1049h enter alt screen, ESC[?1049l exit.  This simulates
        # what a TUI app does without needing one installed.
        & $PSMUX send-keys -t $SESSION ([char]27 + "[?1049h" + "TUI_VISIBLE_TEXT" + [char]27 + "[?1049l") 2>&1 | Out-Null
        Start-Sleep -Seconds 1
        # Step 3: emit 50 more lines AFTER exiting alt screen.
        & $PSMUX send-keys -t $SESSION '1..50 | ForEach-Object { "post $_" }' Enter 2>&1 | Out-Null
        if (Wait-Output -Target $SESSION -Marker "post 49" -TimeoutMs 30000) {
            Start-Sleep -Seconds 1
            $deep = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
            $preCount = ([regex]::Matches($deep, '(?m)^pre (\d+)\b')).Count
            $postCount = ([regex]::Matches($deep, '(?m)^post (\d+)\b')).Count
            Write-Info "T2: pre-alt lines retained: $preCount of 50, post-alt: $postCount of 50"
            if ($preCount -ge 45 -and $postCount -ge 45) {
                Write-Pass "T2: scrollback survives alt-screen enter/exit"
            } else {
                Write-Fail "T2: scrollback corrupted by alt-screen excursion (pre=$preCount post=$postCount)"
            }
        }
    }
}
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

# ── T3: CODEX EXEC — exact CXwudi scenario ──────────────────────────
Write-Host "`n=== T3: CXwudi's literal repro — codex exec emits 200 lines ===" -ForegroundColor Cyan
$codexExe = (Get-Command codex -EA SilentlyContinue).Source
if (-not $codexExe) {
    Write-Info "T3: skipping — codex not found on PATH"
} else {
    Reset-Server
    $SESSION = "iss88_codex"
    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    if (-not (Wait-Prompt -Target $SESSION)) {
        Write-Fail "T3: shell not ready"
    } else {
        Write-Pass "T3: shell ready"
        # `codex exec` runs non-interactively and writes to stdout — no
        # alt screen.  Ask it to print 200 numbered lines.  We use a
        # deterministic prompt that asks for plain stdout, no tooling.
        # Use a marker that's unique enough to grep without false hits.
        $prompt = "Print exactly 200 lines of the form 'codex line N' where N is 1 to 200, one per line, nothing else, no commentary, no markdown."
        Write-Info "T3: launching codex exec inside pane (this may take 30-60s)..."
        # Use single quotes around the prompt; PowerShell will not
        # interpolate, and the prompt is sent verbatim to send-keys.
        & $PSMUX send-keys -t $SESSION "Set-Location 'c:\cctest'" Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
        & $PSMUX send-keys -t $SESSION ("codex exec --skip-git-repo-check `"$prompt`"") Enter 2>&1 | Out-Null

        # Wait for codex to finish (look for the last marker), or for codex to
        # report an error (usage limit, no login, network): then nothing more
        # comes and the count cannot be measured. The wait stays under the
        # runner's 240 s suite limit so a slow model skips instead of timing out.
        $codexDone = Wait-Output -Target $SESSION -Marker "codex line 199|(?m)^ERROR:" -TimeoutMs 180000
        $codexCap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($codexDone -and $codexCap -notmatch "codex line 199" -and $codexCap -match "(?m)^ERROR:.*") {
            Write-Info "T3: codex reported an error, skipping the count assertion: $($Matches[0].Trim())"
        } elseif ($codexDone) {
            Start-Sleep -Seconds 3
            $deep = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
            $matches = [regex]::Matches($deep, '(?m)^codex line (\d+)\b')
            $count = $matches.Count
            if ($count -gt 0) {
                $nums = $matches | ForEach-Object { [int]$_.Groups[1].Value }
                $min = ($nums | Measure-Object -Minimum).Minimum
                $max = ($nums | Measure-Object -Maximum).Maximum
                Write-Info "T3: captured $count codex lines, range [$min..$max]"
            }
            if ($count -ge 195) {
                Write-Pass "T3: codex exec output survives in scrollback ($count of 200)"
            } else {
                Write-Fail "T3: BUG CONFIRMED — only $count of 200 codex lines retained"
            }
        } else {
            Write-Info "T3: codex did not finish within 3 minutes (skipping count assertion)"
            $cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
            $tail = $cap.Substring([Math]::Max(0, $cap.Length - 500))
            Write-Info "T3: pane tail:`n$tail"
        }
    }
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Reset-Server
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
