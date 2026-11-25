# Issue #1078: a bare `%N` / `@N` that several sessions own resolved to one of
# them (the newest) at exit 0.
#
# psmux runs one server per session and each numbers its panes and windows from
# 1, so with sessions a, b and c alive `%1` and `@1` exist three times. A bare
# `-t %1` from outside any pane has no session to go by, and the client routed
# it by recency: `display-message -p -t %1 '#S'` printed `c` at rc 0 and
# `send-keys -t %1` typed into session c, whatever the caller meant.
#
# tmux numbers ids server wide, so its bare id always names one pane. psmux
# cannot (one server per session), so the rule is:
#   * outside any pane, an id several sessions own is refused (rc 1, stderr
#     "ambiguous pane id %1 ..."), for EVERY command that takes it as -t or -s,
#     and nothing is done anywhere
#   * an id exactly one session owns goes to that session
#   * inside a pane ($TMUX names its server) the pane's own session wins when
#     it owns the id: `send-keys -t $env:TMUX_PANE`, and the Claude Code agent
#     teams idiom `split-window -P -F '#{pane_id}'` then `send-keys -t %N`,
#     keep working with the same ids alive in other sessions
#   * qualified targets (`=a:0.0`, `a:@1`) are untouched
#
# Own -L namespace and PSMUX_DATA_DIR; set PSMUX_TEST_BIN to test a build.

$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Section($m) { Write-Host ""; Write-Host $m -ForegroundColor Cyan }
Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_TARGET_SESSION') {
    Remove-Item "Env:$v" -ErrorAction SilentlyContinue
}
$env:PSMUX_NO_WARM = '1'
$rig = Join-Path $env:TEMP ("psmux1078-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$root = Join-Path $rig 'data'
New-Item -ItemType Directory -Force -Path $rig, $root | Out-Null
$env:PSMUX_DATA_DIR = $root
$conf = Join-Path $rig 'empty.conf'
Set-Content -Path $conf -Value '' -Encoding ascii
$NS = 'i1078' + [guid]::NewGuid().ToString('N').Substring(0, 6)

function Px {
    param([string[]]$a)
    $ef = Join-Path $rig 'err.txt'
    $out = & $PSMUX -L $NS -f $conf @a 2>$ef
    $rc = $LASTEXITCODE
    $err = if (Test-Path $ef) { (Get-Content $ef -Raw) } else { '' }
    [pscustomobject]@{ out = (($out | Out-String).Trim()); err = ("$err").Trim(); rc = $rc }
}
function Marker-Sessions($mark) {
    $hit = @()
    foreach ($s in 'a', 'b', 'c') {
        $cap = (Px @('capture-pane', '-p', '-t', "=${s}:")).out
        if ($cap -match [regex]::Escape($mark)) { $hit += $s }
    }
    $hit -join ','
}
function Wait-Capture($target, $mark, $ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        if ((Px @('capture-pane', '-p', '-t', $target)).out -match [regex]::Escape($mark)) { return $true }
        Start-Sleep -Milliseconds 300
    }
    $false
}
function Wait-File($path, $ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        if ((Test-Path $path) -and (Get-Content $path -Raw) -match 'DONE') { return $true }
        Start-Sleep -Milliseconds 200
    }
    $false
}

try {
    foreach ($s in 'a', 'b', 'c') {
        Px @('new-session', '-d', '-s', $s) | Out-Null
        Start-Sleep -Milliseconds 700
    }
    $rows = (Px @('list-panes', '-a', '-F', '#{session_name} #{window_id} #{pane_id}')).out
    Write-Info "rig:`n$rows"
    if (($rows -split "`n").Count -ne 3 -or $rows -notmatch 'a @1 %1' -or $rows -notmatch 'c @1 %1') {
        Write-Fail "rig not as expected"; throw 'rig'
    }

    Write-Section "1. Outside any pane: an id three sessions own is refused"
    foreach ($pair in @(@('%1', 'pane'), @('@1', 'window'), @('@1.%1', 'window'), @('@1.0', 'window'))) {
        $id, $kind = $pair
        $r = Px @('display-message', '-p', '-t', $id, '#{session_name}')
        $want = $id -replace '\..*$', ''
        if ($r.rc -eq 1 -and -not $r.out -and $r.err -match "ambiguous $kind id $([regex]::Escape($want))") {
            Write-Pass "display -t $id refused: $($r.err)"
        } else {
            Write-Fail "display -t $id -> out='$($r.out)' err='$($r.err)' rc=$($r.rc) (master printed 'c' at rc 0)"
        }
    }
    $r = Px @('send-keys', '-t', '%1', 'echo AMBIG_SEND_1078', 'Enter')
    Start-Sleep -Milliseconds 1500
    $hit = Marker-Sessions 'AMBIG_SEND_1078'
    if ($r.rc -eq 1 -and $r.err -match 'ambiguous pane id %1' -and -not $hit) {
        Write-Pass "send-keys -t %1 refused and reached no session"
    } else {
        Write-Fail "send-keys -t %1 rc=$($r.rc) err='$($r.err)' reached '$hit'"
    }

    # Commands off the #545 validation list used to go by recency too.
    $more = @(
        @('set-option', '-p', '-t', '%1', '@m1078', 'x'),
        @('show-options', '-p', '-t', '%1'),
        @('set-option', '-w', '-t', '@1', '@w1078', 'y'),
        @('respawn-pane', '-k', '-t', '%1'),
        @('run-shell', '-t', '%1', 'echo hi'),
        @('kill-window', '-t', '@1'),
        @('break-pane', '-d', '-s', '%1'),
        @('join-pane', '-d', '-s', '%1'),
        @('swap-pane', '-d', '-s', '%1', '-t', '%1')
    )
    foreach ($c in $more) {
        $r = Px $c
        if ($r.rc -eq 1 -and $r.err -match 'ambiguous (pane|window) id') {
            Write-Pass "$($c -join ' ') refused"
        } else {
            Write-Fail "$($c -join ' ') -> out='$($r.out)' err='$($r.err)' rc=$($r.rc)"
        }
    }
    $after = (Px @('list-panes', '-a', '-F', '#{session_name} #{window_id} #{pane_id} [#{@m1078}]')).out
    $wins = (Px @('list-windows', '-a', '-F', '#{session_name} #{window_id} [#{@w1078}]')).out
    $want = (($rows -split "`r?`n") | ForEach-Object { "$_ []" }) -join "`n"
    if ((($after -split "`r?`n") -join "`n") -eq $want -and (($wins -split "`r?`n") -join "`n") -eq "a @1 []`nb @1 []`nc @1 []") {
        Write-Pass "no session changed by the refused commands"
    } else {
        Write-Fail "state changed:`n$after`n$wins"
    }

    # On a build without the fix the commands above really ran (kill-window
    # killed c, break-pane renumbered b): start the later sections from a
    # fresh rig so they measure their own case.
    Px @('kill-server') | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($s in 'a', 'b', 'c') {
        Px @('new-session', '-d', '-s', $s) | Out-Null
        Start-Sleep -Milliseconds 700
    }

    Write-Section "2. Unique and qualified ids still resolve"
    $p2 = (Px @('split-window', '-d', '-t', '=a:', '-P', '-F', '#{pane_id}')).out
    Write-Info "split in a gave $p2"
    $r = Px @('display-message', '-p', '-t', $p2, '#{session_name}')
    if ($p2 -eq '%2' -and $r.rc -eq 0 -and $r.out -eq 'a') { Write-Pass "unique $p2 -> a" } else { Write-Fail "unique $p2 -> '$($r.out)' '$($r.err)' rc=$($r.rc)" }
    $r = Px @('send-keys', '-t', $p2, 'echo UNIQUE_SEND_1078', 'Enter')
    if ($r.rc -eq 0 -and (Wait-Capture $p2 'UNIQUE_SEND_1078')) { Write-Pass "send-keys -t $p2 reached a" } else { Write-Fail "send-keys -t $p2 rc=$($r.rc) err='$($r.err)'" }
    foreach ($t in '=b:0.0', 'b:@1', '=b:') {
        $r = Px @('display-message', '-p', '-t', $t, '#{session_name}')
        if ($r.rc -eq 0 -and $r.out -eq 'b') { Write-Pass "qualified $t -> b" } else { Write-Fail "qualified $t -> '$($r.out)' '$($r.err)' rc=$($r.rc)" }
    }

    Write-Section "3. `$TMUX names a session: its own ids win"
    $portB = (Get-Content -LiteralPath (Join-Path $root "${NS}__b.port") -Raw).Trim()
    $env:TMUX = "fixture,$portB,0"
    try {
        foreach ($id in '%1', '@1') {
            $r = Px @('display-message', '-p', '-t', $id, '#{session_name}')
            if ($r.rc -eq 0 -and $r.out -eq 'b') { Write-Pass "TMUX=b: $id -> b" } else { Write-Fail "TMUX=b: $id -> '$($r.out)' '$($r.err)' rc=$($r.rc)" }
        }
        $r = Px @('display-message', '-p', '-t', '%2', '#{session_name}')
        if ($r.rc -eq 0 -and $r.out -eq 'a') { Write-Pass "TMUX=b: %2 (only a owns it) -> a" } else { Write-Fail "TMUX=b: %2 -> '$($r.out)' '$($r.err)' rc=$($r.rc)" }
    } finally { Remove-Item Env:TMUX -ErrorAction SilentlyContinue }

    Write-Section "4. Real pane in session b: TMUX_PANE and the agent teams idiom"
    $inner = Join-Path $rig 'inner.ps1'
    $res = Join-Path $rig 'inner.txt'
    @"
`$o = @()
`$o += 'PANE=' + `$env:TMUX_PANE
`$o += 'SELF=' + (& '$PSMUX' -L $NS display-message -p -t `$env:TMUX_PANE '#{session_name}' 2>&1)
`$o += 'BARE1=' + (& '$PSMUX' -L $NS display-message -p -t '%1' '#{session_name}' 2>&1)
`$o += 'WIN1=' + (& '$PSMUX' -L $NS display-message -p -t '@1' '#{session_name}' 2>&1)
`$new = & '$PSMUX' -L $NS split-window -d -P -F '#{pane_id}'
`$o += 'NEW=' + `$new
& '$PSMUX' -L $NS send-keys -t `$new 'echo TEAMMATE_1078' Enter
`$o += 'SENDRC=' + `$LASTEXITCODE
`$o += 'NEWSESS=' + (& '$PSMUX' -L $NS display-message -p -t `$new '#{session_name}' 2>&1)
`$o += 'DONE'
Set-Content -LiteralPath '$res' -Value `$o -Encoding utf8
"@ | Set-Content -LiteralPath $inner -Encoding utf8
    Px @('send-keys', '-t', '=b:0.0', "& '$inner'", 'Enter') | Out-Null
    if (Wait-File $res) {
        $kv = @{}
        foreach ($l in Get-Content $res) { $k, $v = $l -split '=', 2; $kv[$k] = "$v".Trim() }
        Write-Info ((Get-Content $res) -join ' | ')
        if ($kv.PANE -eq '%1' -and $kv.SELF -eq 'b') { Write-Pass "TMUX_PANE %1 -> own session b" } else { Write-Fail "TMUX_PANE -> '$($kv.SELF)'" }
        if ($kv.BARE1 -eq 'b') { Write-Pass "bare %1 in pane -> b" } else { Write-Fail "bare %1 in pane -> '$($kv.BARE1)'" }
        if ($kv.WIN1 -eq 'b') { Write-Pass "bare @1 in pane -> b" } else { Write-Fail "bare @1 in pane -> '$($kv.WIN1)'" }
        # a already owns %2, so the new pane id collides across sessions.
        if ($kv.NEW -eq '%2' -and $kv.SENDRC -eq '0' -and $kv.NEWSESS -eq 'b') {
            Write-Pass "split-window -P gave %2 (also in a), send-keys/display on it stay in b"
        } else { Write-Fail "agent idiom: NEW=$($kv.NEW) SENDRC=$($kv.SENDRC) NEWSESS=$($kv.NEWSESS)" }
        $inB = Wait-Capture '=b:0.1' 'TEAMMATE_1078'
        $capA = (Px @('capture-pane', '-p', '-t', '=a:0.1')).out
        if ($inB -and $capA -notmatch 'TEAMMATE_1078') { Write-Pass "teammate text landed in b's new pane only" } else { Write-Fail "teammate text: b has $inB a has $($capA -match 'TEAMMATE_1078')" }
    } else {
        Write-Fail "inner pane script did not finish"
    }
} catch {
    Write-Fail "exception: $_"
} finally {
    & $PSMUX -L $NS kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $rig -ErrorAction SilentlyContinue
}
Write-Host ""
Write-Host "Results: $script:Pass passed, $script:Fail failed" -ForegroundColor $(if ($script:Fail) { 'Red' } else { 'Green' })
exit $(if ($script:Fail) { 1 } else { 0 })
