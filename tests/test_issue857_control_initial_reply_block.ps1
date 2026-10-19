# Issue #857: in control mode tmux answers the command on the command line
# (`-C new-session -A`, `-C attach`) with a reply block of its own, flagged 0,
# before anything else. Replies to commands read from stdin are flagged 1 and
# numbered after it. psmux sent no block for the command line command (a blank
# line in -C, and a flag 0 block numbered 1 in -CC that the first stdin reply
# reused), so a client pairing replies with commands in order was off by one.
# psmux also echoed every stdin line before its %begin, which tmux never does
# (a terminal echoes for itself, a pipe shows nothing).
#
# tmux 3.4 (WSL), stdin a pipe:
#   -C new-session -A -s t   %begin T 264 0 / %end T 264 0 / notifications /
#                            %begin T 270 1 / from-stdin / %end T 270 1 / %exit
#   -C attach -t nosuch      %begin T 291 0 / can't find session: nosuch /
#                            %error T 291 0 / %exit, exit 1, stderr empty
#   -C new -s e (exists)     same with `duplicate session: e`
#   -CC                      \033P1000p directly in front of the first %begin
#
# Raw stream parsing only. Everything runs under a private -L namespace and a
# private PSMUX_DATA_DIR, and is cleaned up with `psmux -L <ns> kill-server`.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path ([IO.Path]::GetTempPath()) ("psmux_i857_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
}
$script:Passed = 0
$script:Failed = 0
function Check($cond, $msg, $detail = '') {
    if ($cond) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:Passed++ }
    else { Write-Host "  [FAIL] $msg $detail" -ForegroundColor Red; $script:Failed++ }
}
function Show([string]$s) { ($s -replace "`e", '\e' -replace "`n", '\n') }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
Write-Host "data dir: $env:PSMUX_DATA_DIR"
$ns = 'i857_' + [guid]::NewGuid().ToString('N').Substring(0, 8)

# Run a control client with stdin a pipe: wait, send $Lines, wait, close stdin,
# collect all of stdout as raw text.
function Invoke-CC([string]$CcArgs, [string[]]$Lines, [int]$StartMs = 2500) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $PSMUX
    $psi.Arguments = "-L $ns $CcArgs"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $errTask = $p.StandardError.ReadToEndAsync()
    $p.StandardInput.NewLine = "`n"
    Start-Sleep -Milliseconds $StartMs
    try {
        foreach ($l in $Lines) { $p.StandardInput.WriteLine($l) }
        $p.StandardInput.Flush()
        Start-Sleep -Milliseconds 1200
        $p.StandardInput.Close()
    } catch {}
    if (-not $p.WaitForExit(10000)) { try { $p.Kill() } catch {} }
    $p.WaitForExit()
    [void]$outTask.Wait(3000); [void]$errTask.Wait(3000)
    return [pscustomobject]@{ Out = $outTask.Result; Err = $errTask.Result; Code = $p.ExitCode }
}

# Lines of the stream that are not %output (pane output is timing dependent).
function Protocol-Lines([string]$text) {
    @($text -split "`n" | ForEach-Object { $_.TrimEnd("`r") } | Where-Object { $_ -notmatch '^%output ' })
}

# The guard lines (%begin / %end / %error) parsed into objects.
function Guards([string[]]$lines) {
    $i = 0
    foreach ($l in $lines) {
        if ($l -match '^(?:\eP1000p)?%(begin|end|error) (\d+) (\d+) (\d+)$') {
            [pscustomobject]@{ Kind = $Matches[1]; T = $Matches[2]; N = [long]$Matches[3]; F = [int]$Matches[4]; Idx = $i }
        }
        $i++
    }
}

# Common assertions for a successful command line command plus one stdin
# command whose reply body is $Body.
function Check-Success($name, $r, [string]$Sent, [string]$Body, [bool]$Dcs = $false) {
    $lines = Protocol-Lines $r.Out
    $g = @(Guards $lines)
    Check ($r.Code -eq 0) "$name exits 0" "(code $($r.Code), err '$($r.Err.Trim())')"
    if ($Dcs) {
        Check ($r.Out.StartsWith("`eP1000p%begin ")) "$name stdout starts with the DCS opener directly followed by %begin" "(head '$(Show $r.Out.Substring(0, [Math]::Min(40, $r.Out.Length)))')"
        Check ($r.Out.EndsWith("`e\")) "$name stdout ends with ST"
    } else {
        Check ($r.Out.StartsWith("%begin ")) "$name stdout starts with %begin (no blank line, no echo)" "(head '$(Show $r.Out.Substring(0, [Math]::Min(40, $r.Out.Length)))')"
    }
    Check ($g.Count -ge 4) "$name has two reply blocks" "(guards $($g.Count): $(Show $r.Out))"
    if ($g.Count -lt 4) { return }
    Check ($g[0].Kind -eq 'begin' -and $g[0].F -eq 0 -and $g[0].Idx -eq 0) "$name first line of the stream is %begin flagged 0"
    Check ($g[1].Kind -eq 'end' -and $g[1].F -eq 0 -and $g[1].N -eq $g[0].N -and $g[1].T -eq $g[0].T -and $g[1].Idx -eq 1) "$name the flag 0 block is closed by %end with the same time and number, nothing inside it"
    Check ($g[2].Kind -eq 'begin' -and $g[2].F -eq 1) "$name the stdin reply is flagged 1"
    Check ($g[2].N -gt $g[0].N) "$name the stdin reply is numbered after the command line reply ($($g[0].N) then $($g[2].N))"
    Check ($g[3].Kind -eq 'end' -and $g[3].F -eq 1 -and $g[3].N -eq $g[2].N) "$name the stdin reply ends with %end flagged 1"
    Check ($lines[$g[2].Idx + 1] -eq $Body) "$name the stdin reply carries its output" "(got '$($lines[$g[2].Idx + 1])')"
    Check (-not ($lines -contains $Sent)) "$name the stdin command line is not echoed"
    Check (@($g | Where-Object { $_.F -eq 0 }).Count -eq 2) "$name exactly one flag 0 block"
}

function Check-Failure($name, $r, [string]$Msg, [bool]$Dcs = $false) {
    $pre = if ($Dcs) { "`eP1000p" } else { '' }
    $post = if ($Dcs) { "`e\" } else { '' }
    $re = '^' + [regex]::Escape($pre) + '%begin (\d+) 1 0\n' + [regex]::Escape($Msg) + '\n%error \1 1 0\n%exit\n' + [regex]::Escape($post) + '$'
    $norm = $r.Out -replace "`r`n", "`n"
    Check ($norm -match $re) "$name stdout is the flag 0 %error block for '$Msg' then %exit" "(got '$(Show $norm)')"
    Check ($r.Code -eq 1) "$name exits 1" "(code $($r.Code))"
    Check ([string]::IsNullOrWhiteSpace($r.Err)) "$name writes nothing to stderr" "(stderr '$($r.Err.Trim())')"
}

try {
    Write-Host "`n=== 1. -C new-session -A -s t, no session yet ==="
    $r = Invoke-CC "-C new-session -A -s t" @('display-message -p from-stdin') 3500
    Check-Success "new -A (fresh):" $r 'display-message -p from-stdin' 'from-stdin'

    & $PSMUX -L $ns new-session -d -s e 2>&1 | Out-Null
    for ($i = 0; $i -lt 50; $i++) { & $PSMUX -L $ns has-session -t e 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 100 }

    Write-Host "`n=== 2. -C new-session -A -s e, session exists ==="
    $r = Invoke-CC "-C new-session -A -s e" @('display-message -p from-stdin')
    Check-Success "new -A (existing):" $r 'display-message -p from-stdin' 'from-stdin'

    Write-Host "`n=== 3. -C attach -t e, several stdin commands ==="
    $r = Invoke-CC "-C attach -t e" @('display-message -p one', 'bogus-cmd-857', 'display-message -p three')
    Check-Success "attach:" $r 'display-message -p one' 'one'
    $lines = Protocol-Lines $r.Out
    $g = @(Guards $lines)
    $stdinGuards = @($g | Where-Object { $_.F -eq 1 })
    Check ($stdinGuards.Count -eq 6) "attach: three stdin replies, all flagged 1" "(got $($stdinGuards.Count))"
    if ($stdinGuards.Count -eq 6) {
        $nums = @($stdinGuards[0].N, $stdinGuards[2].N, $stdinGuards[4].N)
        Check ($nums[0] -lt $nums[1] -and $nums[1] -lt $nums[2]) "attach: stdin replies are numbered in order ($($nums -join ' '))"
        Check ($stdinGuards[3].Kind -eq 'error') "attach: the unknown stdin command is %error flagged 1"
        Check ($lines -contains 'parse error: unknown command: bogus-cmd-857') "attach: tmux's parse error text"
        Check ($lines -contains 'three') "attach: the command after the failing one still runs"
    }
    foreach ($s in 'display-message -p one', 'bogus-cmd-857', 'display-message -p three') {
        Check (-not ($lines -contains $s)) "attach: '$s' is not echoed"
    }

    Write-Host "`n=== 4. -C attach -t nosuch (a server exists) ==="
    $r = Invoke-CC "-C attach -t nosuch" @('display-message -p from-stdin') 1500
    Check-Failure "attach nosuch:" $r "can't find session: nosuch"

    Write-Host "`n=== 5. -C new-session -s e (duplicate) ==="
    $r = Invoke-CC "-C new-session -s e" @('display-message -p from-stdin') 1500
    Check-Failure "new duplicate:" $r "duplicate session: e"

    Write-Host "`n=== 6. -CC attach -t e (iTerm2 path) ==="
    $r = Invoke-CC "-CC attach -t e" @('display-message -p from-stdin')
    Check-Success "-CC attach:" $r 'display-message -p from-stdin' 'from-stdin' $true

    Write-Host "`n=== 7. -CC attach -t nosuch ==="
    $r = Invoke-CC "-CC attach -t nosuch" @('display-message -p from-stdin') 1500
    Check-Failure "-CC attach nosuch:" $r "can't find session: nosuch" $true

    Write-Host "`n=== 8. Without -C the errors still go to stderr ==="
    $o = & $PSMUX -L $ns attach -t nosuch 2>&1 | Out-String
    Check ($LASTEXITCODE -eq 1 -and $o -match "can't find session: nosuch") "plain attach -t nosuch: stderr message, exit 1" "(got '$($o.Trim())')"

    Write-Host "`n=== 9. Raw TCP handshake ==="
    $base = "${ns}__e"
    $port = [int](Get-Content (Join-Path $env:PSMUX_DATA_DIR "$base.port")).Trim()
    $key = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "$base.key")).Trim()
    foreach ($mode in 'CONTROL_QUIET', 'CONTROL') {
        $c = New-Object System.Net.Sockets.TcpClient('127.0.0.1', $port)
        $st = $c.GetStream()
        $st.ReadTimeout = 300
        $wr = New-Object System.IO.StreamWriter($st); $wr.NewLine = "`n"; $wr.AutoFlush = $true
        $wr.WriteLine("AUTH $key"); $wr.WriteLine($mode)
        $buf = New-Object byte[] 65536; $ms = New-Object System.IO.MemoryStream
        $read = {
            param($ms_)
            $sw = [Diagnostics.Stopwatch]::StartNew()
            while ($sw.ElapsedMilliseconds -lt 1500) {
                try { $n = $st.Read($buf, 0, $buf.Length); if ($n -le 0) { break }; $ms_.Write($buf, 0, $n) } catch { }
            }
        }
        & $read $ms
        $wr.WriteLine('display-message -p tcp857')
        & $read $ms
        $c.Close()
        $txt = [Text.Encoding]::UTF8.GetString($ms.ToArray())
        $lines = Protocol-Lines $txt
        Check ($lines[0] -eq 'OK') "${mode}: auth OK"
        Check ($lines[1] -match '^%begin (\d+) 1 0$' -and $lines[2] -eq "%end $($Matches[1]) 1 0") "${mode}: the flag 0 block numbered 1 comes first" "(got '$($lines[1])' '$($lines[2])')"
        Check ($lines -match '^%begin \d+ 2 1$') "${mode}: the first stdin command is number 2, flag 1"
        $echoed = $lines -contains 'display-message -p tcp857'
        if ($mode -eq 'CONTROL') {
            Check $echoed "CONTROL (a console client, whose own echo is off) still gets its line back"
        } else {
            Check (-not $echoed) "CONTROL_QUIET (a pipe) gets no echo"
        }
    }
}
finally {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Get-CimInstance Win32_Process |
        Where-Object { $_.CommandLine -like "*-L $ns*" -and $_.ProcessId -ne $PID } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
}

Write-Host "`nPassed: $script:Passed  Failed: $script:Failed"
exit $script:Failed
