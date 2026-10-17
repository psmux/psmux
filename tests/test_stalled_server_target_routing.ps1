# A command carries its own -t target, even when the server is stalled.
#
# THE DEFECT
# ----------
# The CLI route used to apply a -t target as a SEPARATE request: the
# connection thread sent FocusTargetTemp (switch the active window/pane), waited
# up to 5 s for its reply, then sent the command, and the server loop put the
# focus back after "the next request that is not a temp focus", whoever sent
# it.  Any other request landing in between (another client's command, a pane
# output wake, a second targeted command) consumed or overwrote that temporary
# focus, so the command ran against whatever pane was active instead of its
# target.  A stall longer than the 5 s wait made it near certain: every
# connection's focus request queued first and every command after it.  Seen
# first while a 1.3 MB paste stalled the server: `send-keys -t s:0.1` typed
# into pane 0.0.
#
# tmux resolves a command's target when the command runs (cmd-queue.c
# cmdq_fire_command -> cmd_find_target) and nothing another client does can
# redirect it.  psmux now sends the target WITH the request it belongs to
# (CtrlReq::Targeted) and the server resolves, acts and restores in one step.
#
# HOW: the server is started with PSMUX_TEST_STALL_HOOK=1, which enables the
# test only wire command `debug-stall <ms>` (raw TCP, the CLI has no verb).
# Each iteration stalls the server and fires, concurrently, targeted commands
# whose effects are byte exact and attributable:
#   send-keys -t s:0.1 -l {B<i>}      must reach pane B only
#   send-keys -t %<B>  -l {P<i>}      must reach pane B only
#   send-keys -t s:1   -l {W<i>}      must reach pane C (window 1) only
#   select-pane -t s:0.1 -T T<i>      must title pane B, never A
#   kill-pane -t s:0.2                must kill the sacrificial pane, never A/B
# Pane programs are tests\target_routing_probe.cs, which log every byte read.
# Long stalls (7 s) make every CLI give up; short stalls (1.5 s) do not.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_stalled_server_target_routing.ps1
# -StallMs is a comma separated list ("7000,1500,0"); pwsh -File hands an
# array argument over as one string.
param([int]$Iterations = 10, [string]$StallMs = '7000,1500,0')
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "amis_strt$PID" }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR; $savedNoWarm = $env:PSMUX_NO_WARM; $savedHook = $env:PSMUX_TEST_STALL_HOOK
$root = Join-Path $env:TEMP "psmux_strt_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_TEST_STALL_HOOK = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$probe = Join-Path $root "target_routing_probe.exe"
& $csc /nologo /optimize /platform:x64 /out:$probe (Join-Path $PSScriptRoot "target_routing_probe.cs") 2>&1 | Out-Null
if (-not (Test-Path $probe)) { Write-Host "FATAL: could not compile tests\target_routing_probe.cs" -ForegroundColor Red; exit 1 }
Write-Info ("psmux: {0}  ({1})  ns={2}" -f $PSMUX, ((& $PSMUX -V) -join ' '), $NS)

function Wait-Ready([string]$t) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if (((& $PSMUX -L $NS capture-pane -p -t $t 2>&1) -join "`n") -match 'target_routing_probe') { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
function Read-Bytes([string]$file) {
    if (-not (Test-Path $file)) { return '' }
    try { $fs = [IO.File]::Open($file, 'Open', 'Read', 'ReadWrite'); $ms = New-Object IO.MemoryStream; $fs.CopyTo($ms); $fs.Close(); return [Text.Encoding]::ASCII.GetString($ms.ToArray()) } catch { return '' }
}
function Tokens([string]$file) {
    return @([regex]::Matches((Read-Bytes $file), '\{[A-Z]\d+\}') | ForEach-Object { $_.Value })
}
function Send-Stall([string]$Sess, [int]$Ms) {
    $base = Join-Path $env:PSMUX_DATA_DIR "${NS}__$Sess"
    $port = [int](Get-Content "$base.port" -Raw).Trim(); $key = (Get-Content "$base.key" -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new(); $tcp.NoDelay = $true; $tcp.Connect('127.0.0.1', $port)
    $st = $tcp.GetStream(); $w = [IO.StreamWriter]::new($st, [Text.UTF8Encoding]::new($false)); $w.NewLine = "`n"
    $w.WriteLine("AUTH $key"); $w.WriteLine("debug-stall $Ms"); $w.Flush()
    $tcp.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
    [void][IO.StreamReader]::new($st).ReadToEnd()
    $tcp.Close()
}
# Runs each psmux argument list on its own thread, all at once; returns
# @{ args; out; ms } per command.
function Invoke-Concurrent([object[]]$Cmds) {
    $jobs = @()
    foreach ($c in $Cmds) {
        $ps = [PowerShell]::Create()
        [void]$ps.AddScript({
            param($psmux, $ns, $a)
            $sw = [Diagnostics.Stopwatch]::StartNew()
            $o = (& $psmux -L $ns @a 2>&1) -join ' '
            return [pscustomobject]@{ args = ($a -join ' '); out = $o; ms = $sw.ElapsedMilliseconds }
        }).AddArgument($PSMUX).AddArgument($NS).AddArgument([string[]]$c)
        $jobs += @{ PS = $ps; H = $ps.BeginInvoke() }
    }
    $res = @()
    foreach ($j in $jobs) { $res += $j.PS.EndInvoke($j.H); $j.PS.Dispose() }
    return $res
}

$totals = [ordered]@{ wrong_pane_keys = 0; lost_keys = 0; dup_keys = 0; wrong_title = 0; wrong_kill = 0; wrong_display = 0; focus_moved = 0; timed_out = 0; commands = 0 }
try {
    $cases = @(foreach ($m in @($StallMs -split '[,\s]+' | Where-Object { $_ } | ForEach-Object { [int]$_ })) {
        if ($m -ge 5000) { @{ name = "stall $m ms (longer than the CLI and the old 5 s focus wait)"; ms = $m; tag = 'L' } }
        elseif ($m -gt 0) { @{ name = "stall $m ms (no CLI call gives up)"; ms = $m; tag = 'S' } }
        else { @{ name = 'no stall, concurrent targeted commands only'; ms = 0; tag = 'Z' } }
    })
    foreach ($case in $cases) {
        Write-Host "`n=== $($case.name), $Iterations iterations ===" -ForegroundColor Yellow
        for ($i = 1; $i -le $Iterations; $i++) {
            $S = "$($case.tag)$i"; $stop = Join-Path $root "$S.stop"
            $logA = Join-Path $root "$S.A.bin"; $logB = Join-Path $root "$S.B.bin"; $logC = Join-Path $root "$S.C.bin"; $logD = Join-Path $root "$S.D.bin"
            & $PSMUX -L $NS new-session -d -s $S -x 160 -y 40 -- $probe $logA $stop 2>&1 | Out-Null
            & $PSMUX -L $NS split-window -d -t "${S}:0" -- $probe $logB $stop 2>&1 | Out-Null
            & $PSMUX -L $NS split-window -d -t "${S}:0.1" -- $probe $logD $stop 2>&1 | Out-Null
            & $PSMUX -L $NS new-window -d -t $S -- $probe $logC $stop 2>&1 | Out-Null
            $ok = (Wait-Ready "${S}:0.0") -and (Wait-Ready "${S}:0.1") -and (Wait-Ready "${S}:0.2") -and (Wait-Ready "${S}:1.0")
            $ids = @(& $PSMUX -L $NS list-panes -t "${S}:0" -F '#{pane_id}' 2>&1)
            if (-not $ok -or $ids.Count -ne 3) { Write-Fail "$S setup: probes ready=$ok panes=$($ids -join ',')"; continue }
            $idA = $ids[0]; $idB = $ids[1]; $idD = $ids[2]
            $idC = ((& $PSMUX -L $NS display-message -p -t "${S}:1.0" '#{pane_id}') -join '').Trim()
            $before = ((& $PSMUX -L $NS display-message -p -t $S '#{window_index}.#{pane_index}') -join '')

            if ($case.ms -gt 0) { Send-Stall $S $case.ms; Start-Sleep -Milliseconds 300 }
            $cmds = @(
                @('send-keys', '-t', "${S}:0.1", '-l', "{B$i}"),
                @('send-keys', '-t', $idB, '-l', "{P$i}"),
                @('send-keys', '-t', "${S}:1", '-l', "{W$i}"),
                @('select-pane', '-t', "${S}:0.1", '-T', "T$($case.tag)$i"),
                @('kill-pane', '-t', "${S}:0.2"),
                @('display-message', '-p', '-t', "${S}:1", '#{window_index}.#{pane_id}'),
                @('send-keys', '-N', '3', '-t', "${S}:0.1", '-l', "{N$i}")
            )
            $res = Invoke-Concurrent $cmds
            $to = @($res | Where-Object { $_.out -match 'timed out|no response' }).Count
            $totals.timed_out += $to; $totals.commands += $res.Count
            # Let the stall end and every late command run.
            Start-Sleep -Milliseconds ([Math]::Max(500, $case.ms - 3000) + 1500)
            # Nudge every surviving probe with one more byte, sent by pane id
            # after the concurrent phase, and wait until each one has read it.
            # Conhost sometimes keeps the last of several input writes that
            # arrive within a millisecond or so in the pseudoconsole, undelivered,
            # until the NEXT input arrives (psmux's pty trace shows the token
            # written to the right pane's ConPTY; a bare CreatePseudoConsole
            # harness with no psmux stalls the same way). The nudge flushes
            # such a token so the checks below see what psmux actually wrote.
            # It cannot hide a real routing bug: a token psmux never wrote, or
            # wrote to the wrong pane, is still missing or misplaced.
            foreach ($nudgeId in @($idA, $idB, $idC)) {
                & $PSMUX -L $NS send-keys -t $nudgeId -l '.' 2>&1 | Out-Null
            }
            foreach ($log in @($logA, $logB, $logC)) {
                $sw = [Diagnostics.Stopwatch]::StartNew()
                while ($sw.ElapsedMilliseconds -lt 3000 -and -not (Read-Bytes $log).EndsWith('.')) { Start-Sleep -Milliseconds 50 }
            }

            $tA = Tokens $logA; $tB = Tokens $logB; $tC = Tokens $logC; $tD = Tokens $logD
            $panes = @(& $PSMUX -L $NS list-panes -t "${S}:0" -F '#{pane_id}' 2>&1)
            $titleA = ((& $PSMUX -L $NS display-message -p -t $idA '#{pane_title}') -join '')
            $titleB = ((& $PSMUX -L $NS display-message -p -t $idB '#{pane_title}') -join '')
            $after = ((& $PSMUX -L $NS display-message -p -t $S '#{window_index}.#{pane_index}') -join '')

            $problems = @()
            $wrong = @($tA) + @($tD) + @($tC | Where-Object { $_ -ne "{W$i}" }) + @($tB | Where-Object { $_ -notin @("{B$i}", "{P$i}", "{N$i}") })
            if ($wrong.Count) { $problems += "keys in the wrong pane: A[$($tA -join ',')] D[$($tD -join ',')] C[$($tC -join ',')] B[$($tB -join ',')]"; $totals.wrong_pane_keys += $wrong.Count }
            foreach ($want in @(@("{B$i}", $tB, 1), @("{P$i}", $tB, 1), @("{W$i}", $tC, 1), @("{N$i}", $tB, 3))) {
                $n = @($want[1] | Where-Object { $_ -eq $want[0] }).Count
                if ($n -lt $want[2]) { $problems += "$($want[0]) reached its pane $n of $($want[2]) times"; $totals.lost_keys += $want[2] - $n }
                elseif ($n -gt $want[2]) { $problems += "$($want[0]) arrived $n times, wanted $($want[2])"; $totals.dup_keys++ }
            }
            if ($titleB -ne "T$($case.tag)$i" -or $titleA -eq "T$($case.tag)$i") { $problems += "select-pane -T landed wrong: A='$titleA' B='$titleB'"; $totals.wrong_title++ }
            if (($panes -join ',') -ne "$idA,$idB") { $problems += "kill-pane -t ${S}:0.2 left panes [$($panes -join ',')], wanted [$idA,$idB] (the sacrificial pane was $idD)"; $totals.wrong_kill++ }
            $dm = $res[5].out.Trim()
            if ($dm -notmatch 'timed out|no response' -and $dm -notmatch '^1\.%\d+$') { $problems += "display-message -t ${S}:1 answered '$dm', not window 1"; $totals.wrong_display++ }
            if ($after -ne $before) { $problems += "active pane moved from $before to $after"; $totals.focus_moved++ }
            $cli = ($res | ForEach-Object { "{0}={1}ms{2}" -f ($_.args -split ' ')[0], $_.ms, $(if ($_.out) { " '" + $_.out + "'" } else { '' }) }) -join '; '
            if ($problems.Count -eq 0) { Write-Pass "$S every command acted on its own target ($to of $($res.Count) CLI calls timed out)" }
            else { Write-Fail "$S $($problems -join ' | ')"; Write-Info "    CLI: $cli" }

            New-Item -ItemType File -Force $stop | Out-Null
            Start-Sleep -Milliseconds 300
            & $PSMUX -L $NS kill-session -t $S 2>&1 | Out-Null
        }
    }
} finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir; $env:PSMUX_NO_WARM = $savedNoWarm; $env:PSMUX_TEST_STALL_HOOK = $savedHook
}
Write-Host ""
Write-Info ("totals: " + (($totals.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))
Write-Host ("Passed: {0}  Failed: {1}" -f $script:Pass, $script:Fail)
if ($script:Fail -gt 0) { exit 1 }
exit 0
