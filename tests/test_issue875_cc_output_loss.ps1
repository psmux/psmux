# Issue #875: a control client reading its stdout the whole time still lost
# pane output from %output when the pane wrote fast. The pane's own screen
# and history had every line; the client was never told (no %pause, no
# %exit). The pane's output ring kept only its newest 64 KiB and dropped the
# oldest bytes when a read did not fit, and with no TUI client attached the
# server loop that empties it slept up to 50 ms between passes.
#
# tmux keeps a pane's output until every control client has consumed it,
# stops reading the pane when the only clients are control clients that
# cannot take more (server_client_check_pane_buffer), and tells a client
# that falls behind with %pause or %exit too far behind.
#
# This suite:
#   1. one big write (about 1.5 MB, a single WriteFile from PowerShell):
#      every numbered line reaches %output, in order, with no gap
#   2. the same burst while the client does not read for a few seconds:
#      nothing is lost once it reads again, and no %exit
#   3. a control client that attaches is not sent output written before it
#      attached (the old ring handed it up to 64 KiB of stale output)
#   4. typing echo latency in control mode stays low
#
# Private -L namespace and PSMUX_DATA_DIR; cleaned up with
# `psmux -L <ns> kill-server` only, and stray processes are stopped by PID.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path ([IO.Path]::GetTempPath()) ("psmux_i875_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_CONFIG_FILE') {
    [Environment]::SetEnvironmentVariable($v, $null)
}
$script:Passed = 0
$script:Failed = 0
function Check($cond, $msg) {
    if ($cond) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:Passed++ }
    else { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:Failed++ }
}

Write-Host "psmux: $PSMUX"
Write-Host "data dir: $env:PSMUX_DATA_DIR"
$ns = 'i875_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$work = Join-Path $env:PSMUX_DATA_DIR 'work'
New-Item -ItemType Directory -Force $work | Out-Null
$Lines = 20000
$big = Join-Path $work 'big.txt'
$sb = [Text.StringBuilder]::new()
for ($i = 1; $i -le $Lines; $i++) { [void]$sb.Append(('L{0:d6} {1}' -f $i, ('x' * 60))).Append("`r`n") }
[void]$sb.Append("BURST-DONE`r`n")
[IO.File]::WriteAllText($big, $sb.ToString())
# One write of the whole file after a delay, so the client is attached.
$writer = Join-Path $work 'burst.ps1'
Set-Content $writer "Start-Sleep -Seconds `$args[0]; [Console]::Out.Write([IO.File]::ReadAllText('$big')); [Console]::Out.Flush(); Start-Sleep 3600"
Set-Content (Join-Path $work 'test.conf') 'set -g history-limit 50000'

function Start-Cc([string]$session, [string]$flag = '-C') {
    $psi = [Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $ns $flag attach-session -t $session")
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    [Diagnostics.Process]::Start($psi)
}

# %output payloads of the stream, decoded and joined in order.
function Get-PaneText([string]$path) {
    $sbo = [Text.StringBuilder]::new()
    foreach ($l in [IO.File]::ReadAllLines($path)) {
        $m = [regex]::Match($l, '^%(?:output %\d+|extended-output %\d+ \d+ :) (.*)$')
        if (-not $m.Success) { continue }
        [void]$sbo.Append([regex]::Replace($m.Groups[1].Value, '\\([0-7]{3})', { param($x) [string][char][Convert]::ToInt32($x.Groups[1].Value, 8) }))
    }
    $sbo.ToString()
}

function Test-Burst([string]$name, [string]$session, [int]$readDelay) {
    & $PSMUX -L $ns -f (Join-Path $work 'test.conf') new-session -d -s $session -x 80 -y 24 "powershell -NoProfile -ExecutionPolicy Bypass -File `"$writer`" 3" 2>&1 | Out-Null
    $client = Start-Cc $session
    $out = Join-Path $work "$session.out"
    $file = [IO.FileStream]::new($out, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
    # $readDelay seconds without reading: the burst starts at 3 s.
    if ($readDelay -gt 0) { Start-Sleep -Seconds $readDelay }
    $copy = $client.StandardOutput.BaseStream.CopyToAsync($file)
    $deadline = (Get-Date).AddSeconds(40)
    do {
        Start-Sleep -Milliseconds 500
        $file.Flush()
        $rd = [IO.FileStream]::new($out, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite); $done = ([IO.StreamReader]::new($rd)).ReadToEnd() -match "BURST-DONE"; $rd.Close()
    } while (-not $done -and (Get-Date) -lt $deadline)
    Start-Sleep -Milliseconds 500
    $alive = -not $client.HasExited
    try { $client.Kill() } catch {}
    try { $copy.Wait(3000) | Out-Null } catch {}
    $file.Close()
    $hist = (& $PSMUX -L $ns capture-pane -p -t $session -S -60000) -join "`n"
    & $PSMUX -L $ns kill-session -t $session 2>&1 | Out-Null
    $raw = [IO.File]::ReadAllText($out)
    $text = Get-PaneText $out
    # ConPTY may write a space as a cursor move (ESC[1C), so match the number only.
    $nums = [regex]::Matches($text, 'L(\d{6})(?!\d)') | ForEach-Object { [int]$_.Groups[1].Value }
    $histN = @([regex]::Matches($hist, 'L(\d{6}) ') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique).Count
    $uniq = @($nums | Sort-Object -Unique).Count
    $inOrder = $true
    for ($k = 1; $k -lt $nums.Count; $k++) { if ($nums[$k] -lt $nums[$k - 1]) { $inOrder = $false; break } }
    $have = [Collections.Generic.HashSet[int]]::new([int[]]@($nums))
    $missing = @(1..$Lines | Where-Object { -not $have.Contains($_) } | Select-Object -First 5)
    Write-Host "  $name : pane history $histN, %output $uniq of $Lines, client alive $alive"
    Check ($histN -eq $Lines) "$name : the pane history has all $Lines lines (sanity)"
    Check ($uniq -eq $Lines) "$name : every line reached %output ($uniq of $Lines$(if ($missing) { ', first missing ' + ($missing -join ',') }))"
    Check $inOrder "$name : %output lines are in order"
    Check ($text -match 'BURST-DONE') "$name : the end marker arrived"
    Check ($raw -notmatch '(?m)^%exit') "$name : no %exit"
    Check $alive "$name : the client is still attached"
}

try {
    Write-Host "`n[1] one 1.5 MB write, client reading all along"
    Test-Burst 'reading' 'b1' 0

    Write-Host "`n[2] the same burst while the client does not read for 8 s"
    Test-Burst 'stalled reader' 'b2' 8

    Write-Host "`n[3] a control client is not sent output from before it attached"
    $stale = Join-Path $work 'stale.cmd'
    Set-Content $stale "@for /L %%i in (1,1,300) do @echo STALE%%i"
    & $PSMUX -L $ns new-session -d -s s3 -x 80 -y 24 "cmd /k `"$stale`"" 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $c = Start-Cc 's3'
    $out3 = Join-Path $work 's3.out'
    $f3 = [IO.File]::Create($out3)
    $cp3 = $c.StandardOutput.BaseStream.CopyToAsync($f3)
    Start-Sleep -Seconds 1
    & $PSMUX -L $ns send-keys -t s3 'echo FRESH875' Enter
    Start-Sleep -Seconds 2
    try { $c.Kill() } catch {}
    try { $cp3.Wait(3000) | Out-Null } catch {}
    $f3.Close()
    $t3 = Get-PaneText $out3
    $staleN = ([regex]::Matches($t3, 'STALE\d+')).Count
    Check ($t3 -match 'FRESH875') "output written after attach arrives"
    Check ($staleN -eq 0) "no output from before the attach ($staleN STALE lines)"
    & $PSMUX -L $ns kill-session -t s3 2>&1 | Out-Null

    Write-Host "`n[4] typing echo latency in control mode"
    & $PSMUX -L $ns new-session -d -s s4 -x 80 -y 24 cmd 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $c = Start-Cc 's4'
    $c.StandardInput.NewLine = "`n"
    $acc = [Text.StringBuilder]::new()
    $buf = New-Object byte[] 65536
    $stream = $c.StandardOutput.BaseStream
    $pending = $stream.ReadAsync($buf, 0, $buf.Length)
    $lat = @()
    Start-Sleep -Milliseconds 1500
    for ($i = 0; $i -lt 25; $i++) {
        $tok = 'Q{0:d3}Q' -f $i
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $c.StandardInput.WriteLine("send-keys -t s4 -l $tok"); $c.StandardInput.Flush()
        $got = $false
        while ($sw.ElapsedMilliseconds -lt 2000) {
            if ($pending.Wait(5)) {
                $n = $pending.Result
                if ($n -le 0) { break }
                [void]$acc.Append([Text.Encoding]::ASCII.GetString($buf, 0, $n))
                $pending = $stream.ReadAsync($buf, 0, $buf.Length)
                if ($acc.ToString() -match "(?m)^%output %\d+ [^\r\n]*$tok") { $got = $true; break }
            }
        }
        if ($got) { $lat += $sw.Elapsed.TotalMilliseconds }
        Start-Sleep -Milliseconds 300
    }
    try { $c.Kill() } catch {}
    $sorted = @($lat | Sort-Object)
    $med = if ($sorted.Count) { $sorted[[int]($sorted.Count / 2)] } else { 9999 }
    Write-Host ("  echo latency: {0} of 25, median {1:N1} ms, max {2:N1} ms" -f $sorted.Count, $med, $(if ($sorted.Count) { $sorted[-1] } else { 0 }))
    Check ($sorted.Count -eq 25) "every typed token echoed in %output"
    Check ($med -lt 30) "median echo latency under 30 ms"
}
finally {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Get-CimInstance Win32_Process |
        Where-Object { $_.CommandLine -like "*-L $ns *" -and $_.ProcessId -ne $PID } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
}

Write-Host "`nPassed: $script:Passed  Failed: $script:Failed"
exit $(if ($script:Failed -eq 0) { 0 } else { 1 })
