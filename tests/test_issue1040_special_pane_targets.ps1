#!/usr/bin/env pwsh
# Issue #1040: `send-keys -t - 'echo a' Enter` after `split-window -d`.
#
# tmux reads the special tokens of a PANE target against the current window
# (cmd-find.c cmd_find_get_pane_with_window): `-` / `+` are the previous /
# next pane by number, `!` the last pane, `{up-of}` and friends the active
# pane's neighbour, also after a dot (`.-`, `:.+`, `0.{previous}`).
#
# psmux 3.4.0 died in the CLI with "no server running on session '-'";
# master read a bare `-` as the previous WINDOW and so typed into the pane
# the user was in, and still died in the CLI for `select-pane -t -` and
# `0.{previous}`. Runs in its own -L namespace and data dir.

param([string]$Exe = $env:PSMUX_TEST_BIN)

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

if (-not $Exe) { $Exe = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $Exe -or -not (Test-Path $Exe)) { Write-Host "[FATAL] psmux not found" -ForegroundColor Red; exit 1 }
Write-Host "  Binary: $Exe"

$ns = "t1040" + [guid]::NewGuid().ToString("N").Substring(0, 8)
$dataDir = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux-" + $ns)
New-Item -ItemType Directory -Force $dataDir | Out-Null
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $dataDir

function Mux { & $Exe -L $ns @args 2>&1 }
function PaneIndex($t) { (Mux display-message -p -t $t '#{pane_index}' | Out-String).Trim() }
function ActiveIndex { (Mux display-message -p '#{pane_index}' | Out-String).Trim() }

function Wait-Text($target, $text, $timeoutMs = 8000) {
    $deadline = (Get-Date).AddMilliseconds($timeoutMs)
    while ((Get-Date) -lt $deadline) {
        if ((Mux capture-pane -p -t $target | Out-String) -match [regex]::Escape($text)) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Wait-Panes($n) {
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) {
        if (@(Mux list-panes -t s1040 -F '#{pane_id}').Count -eq $n) { return $true }
        Start-Sleep -Milliseconds 150
    }
    return $false
}

try {
    Mux new-session -d -s s1040 -x 120 -y 30 | Out-Null
    $deadline = (Get-Date).AddSeconds(15)
    do {
        Mux has-session -t s1040 | Out-Null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $deadline)

    Write-Host "`n[1] The issue's repro: split-window -d, then send-keys -t -" -ForegroundColor Yellow
    Mux split-window -d | Out-Null
    if (-not (Wait-Panes 2)) { Fail "split-window -d did not make a second pane" }
    $out = Mux send-keys -t - 'echo MARK1040A' Enter
    if ($LASTEXITCODE -eq 0) { Pass "send-keys -t - exits 0" } else { Fail "send-keys -t - rc=$LASTEXITCODE out=$out" }
    if (Wait-Text 's1040:0.1' 'MARK1040A') { Pass "the text landed in the new pane 1" } else { Fail "pane 1 does not show MARK1040A" }
    $p0 = Mux capture-pane -p -t 's1040:0.0' | Out-String
    if ($p0 -notmatch 'MARK1040A') { Pass "the active pane 0 did not get it" } else { Fail "pane 0 got the text (old bug)" }
    if ((ActiveIndex) -eq '0') { Pass "focus stayed on pane 0" } else { Fail "focus moved to $(ActiveIndex)" }

    Write-Host "`n[2] Offsets and neighbours in a three pane window" -ForegroundColor Yellow
    # 0 = top left (active), 1 = top right, 2 = bottom.
    Mux split-window -h -d -t 's1040:0.0' | Out-Null
    if (-not (Wait-Panes 3)) { Fail "second split failed" }
    $cases = [ordered]@{
        '-' = '2'; '+' = '1'; '+2' = '2'; '-2' = '1'
        '{previous}' = '2'; '{next}' = '1'
        '{right-of}' = '1'; '{down-of}' = '2'; '{up-of}' = '2'
        '{bottom}' = '2'; '{top-right}' = '1'
        '.-' = '2'; ':.+' = '1'; 's1040:0.+' = '1'; '0.{previous}' = '2'
    }
    foreach ($k in $cases.Keys) {
        $got = PaneIndex $k
        if ($got -eq $cases[$k]) { Pass "display -t '$k' is pane $got" } else { Fail "display -t '$k' gave '$got', want $($cases[$k])" }
    }

    Write-Host "`n[3] ! / {last} after a real pane switch" -ForegroundColor Yellow
    Mux select-pane -t 2 | Out-Null
    if ($LASTEXITCODE -eq 0 -and (ActiveIndex) -eq '2') { Pass "select-pane -t 2 selects pane 2" } else { Fail "select-pane -t 2 rc=$LASTEXITCODE active=$(ActiveIndex)" }
    foreach ($t in '!', '{last}', ':.!') {
        $got = PaneIndex $t
        if ($got -eq '0') { Pass "display -t '$t' is the last pane 0" } else { Fail "display -t '$t' gave '$got'" }
    }

    Write-Host "`n[4] select-pane with special tokens goes through the CLI" -ForegroundColor Yellow
    $steps = @(@('!', '0'), @('-', '2'), @('+', '0'), @('{right-of}', '1'), @('{down-of}', '2'), @('.-', '1'))
    foreach ($s in $steps) {
        $out = Mux select-pane -t $s[0]
        $rc = $LASTEXITCODE
        $now = ActiveIndex
        if ($rc -eq 0 -and $now -eq $s[1]) { Pass "select-pane -t '$($s[0])' -> pane $now" } else { Fail "select-pane -t '$($s[0])' rc=$rc active=$now want $($s[1]) out=$out" }
    }

    Write-Host "`n[5] Other pane commands" -ForegroundColor Yellow
    Mux select-pane -t 0 | Out-Null
    Mux send-keys -t + 'echo MARK1040B' Enter | Out-Null
    if (Wait-Text 's1040:0.1' 'MARK1040B') { Pass "send-keys -t + reaches pane 1" } else { Fail "send-keys -t + missed pane 1" }
    $before = @(Mux list-panes -t s1040 -F '#{pane_index}:#{pane_id}')
    $victim = (Mux display-message -p -t '-' '#{pane_id}' | Out-String).Trim()
    Mux kill-pane -t - | Out-Null
    $after = (Mux list-panes -t s1040 -F '#{pane_id}' | Out-String)
    if ($LASTEXITCODE -eq 0 -and $after -notmatch [regex]::Escape($victim) -and $after -match '%1\b') { Pass "kill-pane -t - killed $victim, not the active pane" } else { Fail "kill-pane -t - before=$($before -join ',') after=$after" }

    Write-Host "`n[6] Window targets keep their window meaning" -ForegroundColor Yellow
    Mux new-window -d -t s1040 | Out-Null
    Start-Sleep -Milliseconds 500
    $w = (Mux display-message -p -t 's1040:+' '#{window_index}' | Out-String).Trim()
    if ($w -eq '1') { Pass "display -t s1040:+ is the next window" } else { Fail "display -t s1040:+ gave '$w'" }
    Mux send-keys -t 's1040:+' 'echo MARK1040W' Enter | Out-Null
    if (Wait-Text 's1040:1' 'MARK1040W') { Pass "send-keys -t s1040:+ reaches window 1" } else { Fail "send-keys -t s1040:+ missed window 1" }
    if ((Mux capture-pane -p -t 's1040:0' | Out-String) -notmatch 'MARK1040W') { Pass "window 0 did not get it" } else { Fail "window 0 got MARK1040W" }
    Mux select-window -t + | Out-Null
    $w = (Mux display-message -p '#{window_index}' | Out-String).Trim()
    if ($w -eq '1') { Pass "select-window -t + moves to window 1" } else { Fail "select-window -t + left window $w" }
    Mux select-window -t - | Out-Null
    $w = (Mux display-message -p '#{window_index}' | Out-String).Trim()
    if ($w -eq '0') { Pass "select-window -t - moves back to window 0" } else { Fail "select-window -t - left window $w" }

    Write-Host "`n[7] A real session target still routes as a session" -ForegroundColor Yellow
    Mux new-session -d -s other1040 -x 80 -y 20 | Out-Null
    Start-Sleep -Milliseconds 800
    $s = (Mux display-message -p -t other1040 '#{session_name}' | Out-String).Trim()
    if ($s -eq 'other1040') { Pass "display -t other1040 answers from that session" } else { Fail "display -t other1040 gave '$s'" }
} finally {
    & $Exe -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $env:PSMUX_DATA_DIR = $savedDataDir
    Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0) { exit 1 } else { exit 0 }
