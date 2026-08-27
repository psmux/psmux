# Issues #601 and #602: move-window / swap-window.
#
# Measured on 3.3.8 (7657bb3), with tmux 3.4 (`tmux -L parity602`) run on the
# same layouts as the reference. Every expectation below is quoted from tmux.
#
#   #602a  move-window IGNORED -s. With `0:main* 1:lazygit 2:claude-`:
#            psmux move-window -s S:2 -t S:9  ->  1:lazygit 2:claude 9:main*
#            tmux  move-window -s p:2 -t p:9  ->  0:main- 1:lazygit 9:claude*
#          CtrlReq::MoveWindow carried the destination alone, so the handler
#          always moved the ACTIVE window. Exit code was 0 either way.
#
#   #602b  Relative targets were read as absolute. With
#          `0:main 2:lazygit 9:claude 10:d 11:c* 12:b 13:a-`:
#            psmux swap-window -t +1  ->  swapped 11 and 2   (rc 0)
#            psmux swap-window -t +2  ->  swapped 11 and 2   (rc 0, same!)
#            psmux swap-window -t -1  ->  rc 1 "no server running on session '-1'"
#            psmux swap-window -t {last} -> rc 1 "no server running on session '{last}'"
#            tmux  swap-window -t +1  ->  swapped 11 and 12
#            tmux  swap-window -t +2  ->  swapped 11 and 13
#            tmux  swap-window -t -1  ->  swapped 11 and 10
#          Rust's usize parser accepts a leading '+', so "+1" arrived as 1;
#          `win_pos(1).unwrap_or(1)` then fell back to a raw VECTOR position
#          when no window held display index 1. `-1` and `{last}` never left
#          the CLI: they were read as SESSION names.
#
#   #601a  Neither handler marked the client state dirty, so an ATTACHED
#          client's window list stayed on the pre-move contents until some
#          UNRELATED command dirtied it. Measured over a persistent TCP
#          connection (AUTH, PERSISTENT, dump-state; "NC" = no change):
#            select-window  -> client frame converged in ~0.6 s
#            rename-window  -> converged in ~1.1 s
#            swap-window    -> STILL STALE after 5 s
#            move-window    -> STILL STALE after 5 s
#
#   #601b  The other half of #601 (swap-window "losing" the active window's
#          identity) is NOT a bug: psmux already matched tmux, which swaps the
#          two winlinks' window pointers and leaves their INDICES alone, so the
#          active marker stays on its number and names the window that landed
#          there. Asserted here so it cannot regress in either direction.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

# Inherited routing would aim every call at whatever session owns this shell.
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$S = "i601-" + [guid]::NewGuid().ToString('N').Substring(0, 8)

function Kill-Rig { & $PSMUX kill-session -t $S 2>&1 | Out-Null }

# list-windows rendered exactly like the tmux reference captures above.
function Get-Windows {
    $o = & $PSMUX list-windows -t $S -F '#{window_index}:#{window_name}#{?window_active,*,}#{?window_last_flag,-,}' 2>&1
    return ((($o | Out-String) -replace '\s+', ' ').Trim())
}

function Invoke-Psmux($argv) {
    $out = & $PSMUX @argv 2>&1
    return @{ rc = $LASTEXITCODE; out = ((($out | Out-String) -replace '\s+', ' ').Trim()) }
}

function Select-Win($idx) {
    & $PSMUX select-window -t ("{0}:{1}" -f $S, $idx) 2>&1 | Out-Null
    Start-Sleep -Milliseconds 250
}

# Build a session whose windows sit at the given display indices. Gaps are made
# with `move-window -t S:<n>` on the freshly created (and therefore current)
# window, which is the one move-window form that worked on 3.3.8, so the rig
# stands up on the old binary too and the assertions below get to run and fail.
function New-Rig($spec) {
    Kill-Rig
    Start-Sleep -Milliseconds 500
    & $PSMUX new-session -d -s $S -n $spec[0][1] 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    for ($i = 1; $i -lt $spec.Count; $i++) {
        & $PSMUX new-window -t "${S}:" -n $spec[$i][1] 2>&1 | Out-Null
        Start-Sleep -Milliseconds 250
        & $PSMUX move-window -t ("{0}:{1}" -f $S, $spec[$i][0]) 2>&1 | Out-Null
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 300
}

function Test-Case($name, $spec, $selectIdx, $argv, $expectAfter, $expectRc) {
    Write-Head $name
    New-Rig $spec
    foreach ($x in $selectIdx) { Select-Win $x }
    $before = Get-Windows
    Write-Info "before: $before"
    $r = Invoke-Psmux $argv
    $after = Get-Windows
    Write-Info ("psmux " + ($argv -join ' ') + "  rc=$($r.rc)" + $(if ($r.out) { " out='$($r.out)'" } else { "" }))
    if ($after -eq $expectAfter) { Write-Pass "layout matches tmux: $after" }
    else { Write-Fail "expected (tmux) '$expectAfter' but got '$after'" }
    if ($null -ne $expectRc) {
        if ($r.rc -eq $expectRc) { Write-Pass "exit code $($r.rc) matches tmux" }
        else { Write-Fail "expected exit $expectRc, got $($r.rc)" }
    }
    return $r
}

$L3   = @(@(0, 'main'), @(1, 'lazygit'), @(2, 'claude'))
$LG4  = @(@(0, 'main'), @(2, 'lazygit'), @(9, 'claude'), @(10, 'd'))
$LGAP = @(@(0, 'main'), @(2, 'lazygit'), @(9, 'claude'), @(10, 'd'), @(11, 'c'), @(12, 'b'), @(13, 'a'))
$L3b  = @(@(0, 'main'), @(1, 'b'), @(2, 'c'))
$L4   = @(@(0, 'main'), @(1, 'b'), @(2, 'c'), @(3, 'd'))

try {
    # =======================================================================
    # #602a: move-window must move the -s window, not the active one
    # =======================================================================
    $null = Test-Case "#602a move-window -s S:2 -t S:9 (current is 0:main)" `
        $L3 @(0) @('move-window', '-s', "${S}:2", '-t', "${S}:9") `
        "0:main- 1:lazygit 9:claude*" 0

    # Selecting 1 then 0 parks the last-window flag on lazygit, away from the
    # window being moved: tmux destroys and re-creates the moved window's
    # winlink, which pops it off the lastw STACK, and psmux keeps a single last
    # slot, so the two only agree when the flag is not on the mover.
    $null = Test-Case "#602a move-window -d -s S:2 -t S:9 keeps the current window" `
        $L3 @(1, 0) @('move-window', '-d', '-s', "${S}:2", '-t', "${S}:9") `
        "0:main* 1:lazygit- 9:claude" 0

    # =======================================================================
    # #602b: relative and symbolic swap-window targets
    # =======================================================================
    $null = Test-Case "#602b swap-window -t +1 (current 11, next in list is 12)" `
        $LGAP @(11) @('swap-window', '-t', '+1') `
        "0:main 2:lazygit 9:claude 10:d 11:b* 12:c 13:a-" 0

    $null = Test-Case "#602b swap-window -t +2 (current 11, two on is 13)" `
        $LGAP @(11) @('swap-window', '-t', '+2') `
        "0:main 2:lazygit 9:claude 10:d 11:a* 12:b 13:c-" 0

    $null = Test-Case "#602b swap-window -t -1 (current 11, previous is 10)" `
        $LGAP @(11) @('swap-window', '-t', '-1') `
        "0:main 2:lazygit 9:claude 10:c 11:d* 12:b 13:a-" 0

    # The case that proves +N is list order, not arithmetic: index 3 does not
    # exist, so tmux's +1 from window 2 is window 9.
    $null = Test-Case "#602b swap-window -t +1 across a GAP (2 -> 9, not 2 -> 3)" `
        $LG4 @(2) @('swap-window', '-t', '+1') `
        "0:main 2:claude* 9:lazygit 10:d-" 0

    $null = Test-Case "#602b swap-window -t {last} (current 2, last 9)" `
        $LG4 @(9, 2) @('swap-window', '-t', '{last}') `
        "0:main 2:claude* 9:lazygit- 10:d" 0

    # move-window's -t is tmux's CMD_FIND_WINDOW_INDEX, where +N IS arithmetic
    # on the display index: 2 + 1 = 3, an index no window holds.
    $null = Test-Case "#602b move-window -t +1 is INDEX arithmetic (2 -> 3)" `
        $LG4 @(2) @('move-window', '-t', '+1') `
        "0:main 3:lazygit* 9:claude 10:d-" 0

    # =======================================================================
    # #602: refusals must reach the caller with a non-zero exit
    # =======================================================================
    Write-Head "#602 errors and exit codes"
    New-Rig $L3b
    Select-Win 0
    $before = Get-Windows

    $r = Invoke-Psmux @('swap-window', '-t', "${S}:77")
    if ($r.rc -ne 0) { Write-Pass "swap-window -t S:77 exits $($r.rc) (tmux: 1)" }
    else { Write-Fail "swap-window -t S:77 exited 0" }
    if ($r.out -match "can't find window: 77") { Write-Pass "message matches tmux: '$($r.out)'" }
    else { Write-Fail "expected tmux's `"can't find window: 77`", got '$($r.out)'" }

    $r = Invoke-Psmux @('move-window', '-s', "${S}:88", '-t', "${S}:5")
    if ($r.rc -ne 0) { Write-Pass "move-window -s S:88 exits $($r.rc) (tmux: 1)" }
    else { Write-Fail "move-window with an unresolvable -s exited 0" }
    if ($r.out -match "can't find window: 88") { Write-Pass "message matches tmux: '$($r.out)'" }
    else { Write-Fail "expected tmux's `"can't find window: 88`", got '$($r.out)'" }
    if ((Get-Windows) -eq $before) { Write-Pass "a refused move-window changed nothing" }
    else { Write-Fail "a refused move-window still mutated the session: $(Get-Windows)" }

    $r = Invoke-Psmux @('move-window', '-s', "${S}:2", '-t', "${S}:1")
    if ($r.rc -ne 0) { Write-Pass "move-window onto an occupied index exits $($r.rc) (tmux: 1)" }
    else { Write-Fail "move-window onto an occupied index exited 0" }
    if ($r.out -match "index in use: 1") { Write-Pass "message matches tmux: '$($r.out)'" }
    else { Write-Fail "expected tmux's `"index in use: 1`", got '$($r.out)'" }
    if ((Get-Windows) -eq $before) { Write-Pass "the refused move left the layout alone" }
    else { Write-Fail "the refused move mutated the session: $(Get-Windows)" }

    # =======================================================================
    # move-window -k and -r
    # =======================================================================
    $null = Test-Case "move-window -k replaces the window at the destination" `
        $L3b @(0) @('move-window', '-k', '-s', "${S}:2", '-t', "${S}:1") `
        "0:main- 1:c*" 0

    $null = Test-Case "move-window -r renumbers the session contiguously" `
        @(@(0, 'main'), @(5, 'b'), @(9, 'c')) @(9, 5) @('move-window', '-r', '-t', "${S}:") `
        "0:main 1:b* 2:c-" 0

    # =======================================================================
    # #601b: swap-window and the active window's identity (tmux parity)
    # =======================================================================
    $null = Test-Case "#601b swap-window -t S:2 keeps the current window NUMBER" `
        $L3 @(0) @('swap-window', '-t', "${S}:2") `
        "0:claude* 1:lazygit 2:main-" 0

    $null = Test-Case "#601b swap with gaps keeps indices in place" `
        $LG4 @(2) @('swap-window', '-t', "${S}:9") `
        "0:main 2:claude* 9:lazygit 10:d-" 0

    # tmux 3.4's cmd-swap-window.c selects the destination index only WITH -d.
    $null = Test-Case "#601b swap-window -d selects the destination index" `
        $L3b @(0) @('swap-window', '-d', '-s', "${S}:0", '-t', "${S}:2") `
        "0:c- 1:b 2:main*" 0

    $null = Test-Case "swap-window -s A -t B with neither one current" `
        $L4 @(0) @('swap-window', '-s', "${S}:1", '-t', "${S}:3") `
        "0:main* 1:d 2:c 3:b-" 0

    # =======================================================================
    # #601a: an attached client must be told the window list changed
    # =======================================================================
    Write-Head "#601a attached client sees move-window / swap-window"
    New-Rig $L3
    Select-Win 0

    $port = (Get-Content -LiteralPath (Join-Path $psmuxDir "$S.port") -EA SilentlyContinue)
    $key  = (Get-Content -LiteralPath (Join-Path $psmuxDir "$S.key")  -EA SilentlyContinue)
    if (-not $port -or -not $key) {
        Write-Fail "no port/key file for $S under $psmuxDir, cannot test the client refresh"
    } else {
        $tcp = New-Object System.Net.Sockets.TcpClient("127.0.0.1", [int]("$port".Trim()))
        $tcp.NoDelay = $true
        $stream = $tcp.GetStream(); $stream.ReadTimeout = 4000
        $writer = New-Object System.IO.StreamWriter($stream); $writer.AutoFlush = $true
        $reader = New-Object System.IO.StreamReader($stream)
        $writer.WriteLine("AUTH " + "$key".Trim())
        $auth = $reader.ReadLine()
        # A persistent connection is what an attached client holds: the server
        # answers dump-state with "NC" whenever it believes nothing changed.
        $writer.WriteLine("PERSISTENT")
        Start-Sleep -Milliseconds 400

        $script:ClientFrame = "<none>"
        function Read-Frame {
            try { $writer.WriteLine("dump-state") } catch { return "DEAD" }
            for ($j = 0; $j -lt 40; $j++) {
                try { $line = $reader.ReadLine() } catch { return "TIMEOUT" }
                if ($null -eq $line) { return "EOF" }
                if ($line -eq "NC") { return "NC" }
                if ($line.Length -gt 100 -and $line.StartsWith("{")) {
                    $o = $line | ConvertFrom-Json
                    $names = @()
                    foreach ($w in $o.windows) {
                        $names += ("" + $w.idx + ":" + $w.name + $(if ($w.active) { "*" } else { "" }) + $(if ($w.last) { "-" } else { "" }))
                    }
                    $script:ClientFrame = ($names -join " ")
                    return "CHANGED"
                }
            }
            return "NONE"
        }

        function Wait-Converged($expected, $budgetMs) {
            $sw = [Diagnostics.Stopwatch]::StartNew()
            while ($sw.ElapsedMilliseconds -lt $budgetMs) {
                $null = Read-Frame
                if ($script:ClientFrame -eq $expected) { $sw.Stop(); return $sw.ElapsedMilliseconds }
                Start-Sleep -Milliseconds 120
            }
            $sw.Stop(); return -1
        }

        function Test-ClientRefresh($label, $argv) {
            # Settle first: the client frame must already agree with the server.
            $null = Wait-Converged (Get-Windows) 6000
            $r = Invoke-Psmux $argv
            $srv = Get-Windows
            Write-Info ("psmux " + ($argv -join ' ') + " rc=$($r.rc); server now '$srv'")
            $ms = Wait-Converged $srv 6000
            if ($ms -ge 0) { Write-Pass "$label pushed the new window list to the client in $ms ms" }
            else { Write-Fail "$label left the client on '$($script:ClientFrame)' after 6 s while the server had '$srv'" }
        }

        if ($auth -ne "OK") {
            Write-Fail "AUTH refused: '$auth'"
        } else {
            # Control: a command that always dirtied the state.
            Test-ClientRefresh "select-window (control)" @('select-window', '-t', "${S}:1")
            Test-ClientRefresh "swap-window -t S:2"      @('swap-window', '-t', "${S}:2")
            Test-ClientRefresh "swap-window -s S:0 -t S:1" @('swap-window', '-s', "${S}:0", '-t', "${S}:1")
            Test-ClientRefresh "move-window -t S:7"      @('move-window', '-t', "${S}:7")
            Test-ClientRefresh "move-window -s S:7 -t S:4" @('move-window', '-s', "${S}:7", '-t', "${S}:4")
        }
        $tcp.Close()
    }
}
finally {
    Kill-Rig
}

Write-Host ""
Write-Host "Passed: $script:TestsPassed  Failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
