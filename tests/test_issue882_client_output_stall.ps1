# PR #882: an attached client whose terminal stops reading must neither grow
# without bound nor be dropped by the server.
#
# A real server floods a pane while the attached client's stdout pipe is left
# unread for longer than the server's 5 s per client write timeout. Then the
# pipe is drained again. Checked:
#   1. the client's private memory stays small while blocked (master grew by
#      about 330 MiB a second here),
#   2. the server keeps counting the client (list-clients) the whole time and
#      no client-detached hook fires (a blocking reader got the client dropped),
#   3. with destroy-unattached on the session survives the stall,
#   4. output flows again afterwards and a server directed detach exits 0.
#
# Isolation: private PSMUX_DATA_DIR, unique -L namespace, cleanup only with
# `-L <ns> kill-server`, and only processes this script started are stopped.

param([int]$BlockSeconds = 9)
$ErrorActionPreference = 'Stop'

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
$ns = "agt_issue882_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_issue882_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$env:PSMUX_DATA_DIR = $data
Remove-Item Env:\SHELL -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX_PANE -ErrorAction SilentlyContinue

$pass = 0; $fail = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:pass++; Write-Host "  PASS  $name  $detail" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  FAIL  $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>$null }
function ClientCount { @(P list-clients | Where-Object { $_ }).Count }

$client = $null
$drain = $null
try {
    P new-session -d -s s -x 160 -y 50
    for ($i = 0; $i -lt 50 -and -not ((P capture-pane -p -t s) -join '').Trim(); $i++) { Start-Sleep -Milliseconds 200 }
    P set-option -g destroy-unattached on
    $hook = (Join-Path $data 'hook.txt').Replace('\', '/')
    P set-hook -g client-detached "run-shell 'echo detached >> $hook'"
    P send-keys -t s '$i=0; while($true){ $i++; Write-Output (''line '' + $i + '' '' + (''x'' * 120)) }' Enter
    for ($i = 0; $i -lt 50 -and -not ((P capture-pane -p -t s) -join '' -match 'line \d+'); $i++) { Start-Sleep -Milliseconds 200 }

    $psi = [Diagnostics.ProcessStartInfo]::new($exe)
    foreach ($a in @('-L', $ns, 'attach', '-t', 's')) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.Environment['PSMUX_PIPE_VT'] = '1'
    $psi.Environment['TERM'] = 'xterm-256color'
    $client = [Diagnostics.Process]::Start($psi)
    $client.StandardInput.Write("$([char]27)[8;50;160t"); $client.StandardInput.Flush()
    $out = $client.StandardOutput.BaseStream

    # Flowing: read for 2 s so the client is attached and drawing.
    $buf = New-Object byte[] 65536
    $got = 0; $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.ElapsedMilliseconds -lt 2000) {
        # The pane floods, so a synchronous read returns at once.
        $n = $out.Read($buf, 0, $buf.Length)
        if ($n -le 0) { break }
        $got += $n
    }
    Check 'client draws while output flows' ($got -gt 10000) "$got bytes"
    Check 'server counts the client' ((ClientCount) -eq 1) ''

    # Blocked: nobody reads the client's stdout.
    $maxMb = 0; $minClients = 99
    $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.Elapsed.TotalSeconds -lt $BlockSeconds) {
        $client.Refresh()
        $mb = [math]::Round($client.PrivateMemorySize64 / 1MB, 1)
        if ($mb -gt $maxMb) { $maxMb = $mb }
        $c = ClientCount
        if ($c -lt $minClients) { $minClients = $c }
        if ($mb -gt 1024) { break }   # safety cap for the old binary
        Start-Sleep -Milliseconds 500
    }
    Check 'client memory stays bounded while its terminal is blocked' ($maxMb -lt 64) "peak $maxMb MiB"
    Check 'server keeps the client attached through the stall' ($minClients -eq 1) "min list-clients $minClients"

    # Resume reading.
    $drain = $out.CopyToAsync([IO.Stream]::Null)
    Start-Sleep -Seconds 3
    $sessions = (P ls) -join "`n"
    Check 'destroy-unattached session survives the stall' ($sessions -match '^s:') $sessions
    Check 'client still running after output resumes' (-not $client.HasExited) ''
    Check 'server still counts the client after resume' ((ClientCount) -eq 1) ''
    $hookFired = (Test-Path (Join-Path $data 'hook.txt'))
    Check 'no client-detached hook fired' (-not $hookFired) ''
    $reply = (P display-message -p 'alive') -join ''
    Check 'server answers commands' ($reply -eq 'alive') $reply

    P detach-client -s s
    $exited = $client.WaitForExit(8000)
    Check 'server directed detach ends the client cleanly' ($exited -and $client.ExitCode -eq 0) "exit=$(if ($exited) { $client.ExitCode } else { 'running' })"
}
finally {
    if ($client -and -not $client.HasExited) { try { $client.Kill() } catch {} }
    & $exe -L $ns kill-server 2>$null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

Write-Host "issue882: $pass passed, $fail failed"
if ($fail -gt 0) { exit 1 }
