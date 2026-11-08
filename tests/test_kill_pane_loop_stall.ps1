# kill-pane must not hold the server loop, and must still kill the pane's tree.
#
# WHAT WAS REPRODUCED (master 46e75b74, a pane running `ping -n 300` under pwsh,
# 25 kill-pane, a display-message on the same session sent with each kill):
#
#   display-message after kill-pane: p50=32.3 p90=38.7 max=154.2 ms
#   display-message with nothing else going on: p50=1.4 ms
#
#   The server loop ran the pane's whole tree kill itself: kill_process_tree
#   (two CreateToolhelp32Snapshot walks of every process on the machine, 11 to
#   28 ms each on a busy machine) and then, from kill_leaf, kill_node killed the
#   same tree a second time (a third walk that found nothing). Every client of
#   the session waited for that. tmux 3.4 answers a display-message right after
#   kill-pane in the time it takes to start its client (p50 6 ms, WSL).
#
# The tree kill now runs on a worker after the pane is cut out of the layout.
#
# WHAT THIS SUITE PINS
#   * a display-message sent with each kill-pane answers quickly (p90 under
#     $LimitMs, where master sat at 30 to 40 ms and the fix at 1 to 2 ms)
#   * every killed pane's shell and its child (ping.exe) are gone afterwards:
#     moving the kill off the loop must not leave orphans
#
# Children are matched by parent pid AND creation time: a pid in the table can
# be a reused parent pid of an older, unrelated process, which is exactly the
# stale edge the kill guard refuses to follow, so the check must refuse it too.
#
# Isolation: -L with a unique name, a private PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server`.
# Run: pwsh -NoProfile -File tests\test_kill_pane_loop_stall.ps1 [-Exe path\to\psmux.exe]
param(
    [string]$Exe = '',
    [int]$Count = 20,
    [double]$LimitMs = 15
)
if (-not $Exe) { $Exe = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { Join-Path $PSScriptRoot '..\target\release\psmux.exe' } }
if (-not (Test-Path -LiteralPath $Exe)) { Write-Output "psmux binary not found: $Exe"; exit 2 }
$Exe = (Resolve-Path -LiteralPath $Exe).Path

$script:Pass = 0; $script:Fail = 0
function Check([bool]$ok, [string]$name) { if ($ok) { $script:Pass++; Write-Output "PASS: $name" } else { $script:Fail++; Write-Output "FAIL: $name" } }

$Ns = "kpls$PID"
$Root = Join-Path ([IO.Path]::GetTempPath()) "psmux-kpls-$PID"
New-Item -ItemType Directory -Force $Root | Out-Null
$savedData = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $Root
$env:PSMUX_NO_WARM = '1'

function Open([string]$line) {
    $c = [Net.Sockets.TcpClient]::new(); $c.NoDelay = $true; $c.Connect('127.0.0.1', $script:Port)
    $s = $c.GetStream(); $s.ReadTimeout = 15000
    $b = [Text.Encoding]::UTF8.GetBytes("AUTH $script:Key`n$line`n"); $s.Write($b, 0, $b.Length); $c.Client.Shutdown('Send')
    @{ c = $c; s = $s }
}
function Finish($h) { try { [IO.StreamReader]::new($h.s).ReadToEnd() } catch { '' } finally { $h.c.Dispose() } }
function Req([string]$line) { Finish (Open $line) }
function Created([int]$procId) {
    $p = Get-CimInstance Win32_Process -Filter "ProcessId=$procId" -EA SilentlyContinue
    if ($p) { $p.CreationDate } else { $null }
}

$tracked = @(); $dm = @()
try {
    & $Exe -L $Ns new-session -d -s s -x 160 -y 50 2>&1 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path "$Root\$($Ns)__s.key") -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 50 }
    $script:Port = [int](Get-Content "$Root\$($Ns)__s.port"); $script:Key = (Get-Content "$Root\$($Ns)__s.key").Trim()

    for ($i = 1; $i -le $Count; $i++) {
        [void](Req 'split-window -v')
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 15000 -and (Req 'capture-pane -p') -notmatch 'PS [A-Z]:\\') { Start-Sleep -Milliseconds 50 }
        $shell = [int](((Req "display-message -p '#{pane_pid}'") -replace '^OK\n', '').Trim().Trim("'"))
        $shellBorn = Created $shell
        [void](Req 'send-keys "ping -n 300 127.0.0.1" Enter')
        $kid = $null
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while (-not $kid -and $sw.ElapsedMilliseconds -lt 10000) {
            $kid = Get-CimInstance Win32_Process -Filter "ParentProcessId=$shell" |
                Where-Object { $_.Name -eq 'PING.EXE' -and $_.CreationDate -ge $shellBorn } | Select-Object -First 1
            if (-not $kid) { Start-Sleep -Milliseconds 100 }
        }
        $tracked += [pscustomobject]@{ Shell = $shell; ShellBorn = $shellBorn; Kid = $kid.ProcessId; KidBorn = $kid.CreationDate }
        Start-Sleep -Milliseconds 300
        $k = Open 'kill-pane'
        $t = [Diagnostics.Stopwatch]::StartNew()
        [void](Req 'display-message -p alive')
        $dm += $t.Elapsed.TotalMilliseconds
        [void](Finish $k)
    }

    $sorted = @($dm | Sort-Object)
    $p50 = $sorted[[int][Math]::Floor(($sorted.Count - 1) * 0.5)]
    $p90 = $sorted[[int][Math]::Ceiling(($sorted.Count - 1) * 0.9)]
    Write-Output ("display-message with kill-pane: p50={0:N1} p90={1:N1} max={2:N1} ms over {3}" -f $p50, $p90, $sorted[-1], $sorted.Count)
    Check ($p90 -lt $LimitMs) "a display-message sent with kill-pane answers in under $LimitMs ms at p90 (got $([math]::Round($p90, 1)))"
    Check (@($tracked | Where-Object { $_.Kid }).Count -eq $Count) "every pane had its ping.exe child before the kill"

    Start-Sleep -Milliseconds 2000
    $left = @()
    foreach ($t in $tracked) {
        foreach ($pair in @(@($t.Shell, $t.ShellBorn), @($t.Kid, $t.KidBorn))) {
            if (-not $pair[0]) { continue }
            $born = Created $pair[0]
            if ($born -and $born -eq $pair[1]) { $left += $pair[0] }
        }
    }
    Check ($left.Count -eq 0) "no killed pane's shell or ping.exe is still running ($($left.Count) left: $($left -join ' '))"
} finally {
    & $Exe -L $Ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    if ($null -eq $savedData) { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $savedData }
    Remove-Item Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item -Recurse -Force $Root -EA SilentlyContinue
}
Write-Output "passed $($script:Pass), failed $($script:Fail)"
exit $script:Fail
