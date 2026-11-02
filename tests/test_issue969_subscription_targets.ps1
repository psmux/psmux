# Issue #969: `refresh-client -B name:target:format` ignored the target, so
# every subscription was expanded against the active pane and tagged with its
# ids. iTerm2 subscribes once per pane and titles each tab from the value, so
# every tab got the same title.
#
# This suite drives a real `psmux -CC attach` over pipes (as iTerm2 does over
# ssh) on a two window session and checks the %subscription-changed lines
# against tmux 3.8 (monitor.c monitor_check_*, control.c control_sub_change):
#   %N   that pane, with its own window id and index
#   %*   one line per pane
#   @N   that window, pane field "-"
#   ""   the session, "- - -"
#   %99  a pane that does not exist: nothing
#   %x   a target that does not parse: nothing (tmux 3.8 removes instead)
# then that nothing is repeated while nothing changes, that a bare `-B name`
# unsubscribes, and that a change to a pane that is not active is reported
# within the one second check for exactly the subscriptions that name it.
#
# Fully isolated: private -L namespace and PSMUX_DATA_DIR, cleaned up with
# `psmux -L <ns> kill-server` only. Run with -Binary <path to psmux.exe>.

param([string]$Binary)

$ErrorActionPreference = 'Continue'
if ($Binary) { $PSMUX = (Resolve-Path $Binary).Path }
elseif ($env:PSMUX_TEST_BIN) { $PSMUX = $env:PSMUX_TEST_BIN }
else { $PSMUX = (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$utf8 = [System.Text.UTF8Encoding]::new($false)
$tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$work = Join-Path ([System.IO.Path]::GetTempPath()) "psmux_i969_$tag"
New-Item -ItemType Directory -Force $work | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $work 'data'
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = '1'
$ns = "i969_$tag"
Write-Host "psmux: $PSMUX ($(& $PSMUX -V)), namespace $ns"

function Read-Shared([string]$path) {
    try {
        $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
        $ms = [System.IO.MemoryStream]::new(); $fs.CopyTo($ms); $fs.Close()
        return , $ms.ToArray()
    } catch { return , [byte[]]@() }
}
function Get-Subs([string]$path) {
    return @(($utf8.GetString((Read-Shared $path)) -split "`n") |
        ForEach-Object { $_.TrimEnd("`r") } | Where-Object { $_.StartsWith('%subscription-changed ') })
}
function Get-Named([string[]]$lines, [string]$name) {
    return @($lines | Where-Object { $_.StartsWith("%subscription-changed $name ") })
}
function Wait-Until([scriptblock]$cond, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 50 }
    return [bool](& $cond)
}
function Q([string]$s) { "'" + $s + "'" }

$cc = $null
try {
    & $PSMUX -L $ns new-session -d -s t -x 120 -y 30
    if (-not (Wait-Until { & $PSMUX -L $ns has-session -t t 2>$null; $LASTEXITCODE -eq 0 } 10000)) { throw "session did not start" }
    & $PSMUX -L $ns new-window -t t:
    Start-Sleep -Milliseconds 1500
    & $PSMUX -L $ns select-pane -t t:0 -T first
    & $PSMUX -L $ns select-pane -t t:1 -T second
    $rows = @(& $PSMUX -L $ns list-panes -s -t t -F '#{session_id} #{window_id} #{window_index} #{pane_id} #{pane_title}')
    Write-Host "  tree: $($rows -join ' | ')"
    if ($rows.Count -ne 2) { throw "expected 2 panes, got $($rows.Count)" }
    $r0 = $rows[0] -split ' '; $r1 = $rows[1] -split ' '
    $S = $r0[0]; $W0 = $r0[1]; $I0 = $r0[2]; $P0 = $r0[3]
    $W1 = $r1[1]; $I1 = $r1[2]; $P1 = $r1[3]
    $active = (& $PSMUX -L $ns display-message -p -t t '#{pane_id}' | Out-String).Trim()
    Write-Host "  active pane: $active (window $W1 is current)"

    $out = Join-Path $work 'cc.out'; $err = Join-Path $work 'cc.err'
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $ns -CC attach -t t")
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $fo = [System.IO.FileStream]::new($out, 'Create', 'Write', 'ReadWrite', 1)
    $fe = [System.IO.FileStream]::new($err, 'Create', 'Write', 'ReadWrite', 1)
    $cc = [pscustomobject]@{ P = $p; Fo = $fo; Fe = $fe
        T1 = $p.StandardOutput.BaseStream.CopyToAsync($fo, 1); T2 = $p.StandardError.BaseStream.CopyToAsync($fe, 1) }
    function Send-CC([string]$cmd) { $cc.P.StandardInput.Write("$cmd`n"); $cc.P.StandardInput.Flush() }
    Wait-Until { $utf8.GetString((Read-Shared $out)).Contains('%session-changed') } 8000 | Out-Null

    Write-Host "`n[1] one subscription per target form" -ForegroundColor Yellow
    Send-CC "refresh-client -B $(Q "a:${P0}:#{pane_title}")"
    Send-CC "refresh-client -B $(Q "b:${P1}:#{pane_title}")"
    Send-CC "refresh-client -B $(Q 'all:%*:#{pane_title}')"
    Send-CC "refresh-client -B $(Q "w:${W0}:#{window_index}/#{pane_title}")"
    Send-CC "refresh-client -B $(Q 'sess::#{pane_title}')"
    Send-CC "refresh-client -B $(Q 'gone:%99:#{pane_title}')"
    Send-CC "refresh-client -B $(Q 'x:%x:#{pane_title}')"
    Wait-Until { (Get-Subs $out).Count -ge 6 } 4000 | Out-Null
    Start-Sleep -Milliseconds 2500
    $first = Get-Subs $out
    $first | ForEach-Object { Write-Host "    $_" }

    function Expect-Exact([string]$name, [string[]]$want) {
        $got = @(Get-Named $first $name)
        $okCount = $got.Count -eq $want.Count
        $okLines = $okCount -and (@($want | Where-Object { $got -notcontains $_ }).Count -eq 0)
        if ($okLines) { Write-Pass "$name : $($want -join ' + ')" }
        else { Write-Fail "$name : want [$($want -join ' + ')] got [$($got -join ' + ')]" }
    }
    Expect-Exact 'a' @("%subscription-changed a $S $W0 $I0 $P0 : first")
    Expect-Exact 'b' @("%subscription-changed b $S $W1 $I1 $P1 : second")
    Expect-Exact 'all' @("%subscription-changed all $S $W0 $I0 $P0 : first", "%subscription-changed all $S $W1 $I1 $P1 : second")
    Expect-Exact 'w' @("%subscription-changed w $S $W0 $I0 - : $I0/first")
    Expect-Exact 'sess' @("%subscription-changed sess $S - - - : second")
    Expect-Exact 'gone' @()
    Expect-Exact 'x' @()

    Write-Host "`n[2] nothing changes, nothing is reported again" -ForegroundColor Yellow
    $n = $first.Count
    Start-Sleep -Milliseconds 3200
    $again = (Get-Subs $out).Count
    if ($again -eq $n) { Write-Pass "no new lines in 3.2 s with nothing changing ($n total)" }
    else { Write-Fail "repeated lines: $n then $again" }

    Write-Host "`n[3] bare -B a unsubscribes, a rename of the inactive pane reaches exactly a's peers" -ForegroundColor Yellow
    Send-CC "refresh-client -B a"
    Start-Sleep -Milliseconds 300
    $t0 = Get-Date
    & $PSMUX -L $ns select-pane -t $P0 -T renamed
    $arrived = Wait-Until { (Get-Subs $out).Count -ge ($n + 2) } 3000
    $ms = [int]((Get-Date) - $t0).TotalMilliseconds
    Start-Sleep -Milliseconds 1500
    $new = @(Get-Subs $out | Select-Object -Skip $n)
    $new | ForEach-Object { Write-Host "    $_" }
    $wantNew = @("%subscription-changed all $S $W0 $I0 $P0 : renamed", "%subscription-changed w $S $W0 $I0 - : $I0/renamed")
    if ($arrived -and $new.Count -eq 2 -and @($wantNew | Where-Object { $new -notcontains $_ }).Count -eq 0) {
        Write-Pass "rename reported once to all and w within $ms ms, not to a, b or sess"
    } else { Write-Fail "after rename want [$($wantNew -join ' + ')] got [$($new -join ' + ')]" }
    if ($ms -le 2500) { Write-Pass "change reported inside the one second check ($ms ms)" } else { Write-Fail "change took $ms ms" }

    Write-Host "`n[4] a new pane appears under %* only" -ForegroundColor Yellow
    $n2 = (Get-Subs $out).Count
    & $PSMUX -L $ns split-window -t t:0 -d
    Start-Sleep -Milliseconds 1500
    & $PSMUX -L $ns select-pane -t "t:0.1" -T third
    $P2 = (& $PSMUX -L $ns display-message -p -t "t:0.1" '#{pane_id}' | Out-String).Trim()
    Wait-Until { @(Get-Subs $out | Select-Object -Skip $n2 | Where-Object { $_ -like "*$P2 : third" }).Count -ge 1 } 3000 | Out-Null
    Start-Sleep -Milliseconds 1200
    $new2 = @(Get-Subs $out | Select-Object -Skip $n2)
    $new2 | ForEach-Object { Write-Host "    $_" }
    $thirdAll = @($new2 | Where-Object { $_ -eq "%subscription-changed all $S $W0 $I0 $P2 : third" })
    $others = @($new2 | Where-Object { $_ -notlike "%subscription-changed all * $P2 : *" })
    if ($thirdAll.Count -eq 1) { Write-Pass "new pane $P2 reported under all with title third" } else { Write-Fail "new pane under all: [$($new2 -join ' + ')]" }
    if (@($others | Where-Object { $_ -like '%subscription-changed b *' -or $_ -like '%subscription-changed a *' }).Count -eq 0) {
        Write-Pass "a and b are not reported for the new pane"
    } else { Write-Fail "unexpected: [$($others -join ' + ')]" }

    Write-Host "`n[5] #() in a subscription runs nothing and does not stall the server" -ForegroundColor Yellow
    # tmux expands subscriptions with FORMAT_NOJOBS (monitor.c), so #() is empty.
    $n3 = (Get-Subs $out).Count
    Send-CC "refresh-client -B $(Q 'job:%*:<#(ping -n 2 127.0.0.1 >nul & echo RAN969)>#{pane_id}')"
    Start-Sleep -Milliseconds 1500
    $lat = @(1..6 | ForEach-Object {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $PSMUX -L $ns display-message -p -t t 'x' | Out-Null
        [int]$sw.Elapsed.TotalMilliseconds; Start-Sleep -Milliseconds 300 })
    $jobs = @(Get-Subs $out | Select-Object -Skip $n3 | Where-Object { $_ -like '%subscription-changed job *' })
    $jobs | ForEach-Object { Write-Host "    $_" }
    $max = ($lat | Measure-Object -Maximum).Maximum
    Write-Host "    display-message ms with the job subscription: $($lat -join ' ')"
    if ($jobs.Count -eq 3 -and @($jobs | Where-Object { $_ -notmatch ' : <>%\d+$' }).Count -eq 0) {
        Write-Pass "#() expands to nothing for every pane"
    } else { Write-Fail "#() lines: [$($jobs -join ' + ')]" }
    if ($max -lt 700) { Write-Pass "server answers in $max ms max (a 1 s job per pane would hold it seconds)" }
    else { Write-Fail "server stalled: $($lat -join ' ') ms" }
} catch {
    Write-Fail "setup error: $_"
} finally {
    if ($cc) {
        if (-not $cc.P.HasExited) { try { $cc.P.StandardInput.Close() } catch {}; if (-not $cc.P.WaitForExit(3000)) { try { $cc.P.Kill() } catch {} } }
        try { $cc.T1.Wait(2000) | Out-Null; $cc.T2.Wait(2000) | Out-Null } catch {}
        $cc.Fo.Close(); $cc.Fe.Close()
    }
    & $PSMUX -L $ns kill-server 2>$null
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
