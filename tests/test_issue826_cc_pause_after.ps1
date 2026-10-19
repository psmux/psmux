# Issue #826: control mode `refresh-client -f pause-after=1` never paused a
# pane under backlog. A client that stopped reading got no %pause, the server
# kept queuing the pane's output (memory grew without bound) and delivered it
# all on resume, with one %pause only after the output had ended.
#
# tmux (control.c control_check_age): when the OLDEST output of a pane that
# the client was not sent yet is older than pause-after, the pane is paused
# for that client, its backlog discarded, `%pause %N` written, and nothing
# more is sent for it until `refresh-client -A '%N:continue'` (%continue).
#
# This suite drives `psmux -L <ns> -C attach` over pipes like the reporter
# (ssh exec): the harness stops reading the client's stdout for several
# seconds while cmd.exe floods the pane, then reads again.
#
# Everything runs under a private -L namespace and is cleaned up with
# `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($cond, $msg) { if ($cond) { Write-Pass $msg } else { Write-Fail $msg } }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$ns = 'i826_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$work = Join-Path ([System.IO.Path]::GetTempPath()) "psmux_i826"
New-Item -ItemType Directory -Force $work | Out-Null
$big = Join-Path $work "big.txt"
if (-not (Test-Path $big) -or (Get-Item $big).Length -lt 15000000) {
    $sb = New-Object System.Text.StringBuilder
    for ($i = 0; $i -lt 200000; $i++) { [void]$sb.Append("line $i the quick brown fox jumps over the lazy dog 0123456789 abcdefghij`r`n") }
    [IO.File]::WriteAllText($big, $sb.ToString())
}

class CcClient {
    [System.Diagnostics.Process]$Proc
    [System.IO.Stream]$Out
    [System.IO.StreamWriter]$In
    [byte[]]$Buf = (New-Object byte[] 65536)
    [System.IO.MemoryStream]$All = [System.IO.MemoryStream]::new()
    [object]$Pending = $null

    CcClient([string]$bin, [string]$ns, [string]$session) {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $bin
        $psi.Arguments = "-L $ns -C attach -t $session"
        $psi.UseShellExecute = $false
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $this.Proc = [System.Diagnostics.Process]::Start($psi)
        $this.Out = $this.Proc.StandardOutput.BaseStream
        $this.In = $this.Proc.StandardInput
        $this.In.NewLine = "`n"
    }
    [void] Send([string]$line) { $this.In.WriteLine($line); $this.In.Flush() }
    # Read whatever arrives for $ms milliseconds.
    [void] ReadFor([int]$ms) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt $ms) {
            if ($null -eq $this.Pending) { $this.Pending = $this.Out.ReadAsync($this.Buf, 0, $this.Buf.Length) }
            $left = [int][Math]::Max(1, $ms - $sw.ElapsedMilliseconds)
            if (-not $this.Pending.Wait($left)) { return }
            $n = $this.Pending.Result; $this.Pending = $null
            if ($n -le 0) { return }
            $this.All.Write($this.Buf, 0, $n)
        }
    }
    [long] Bytes() { return $this.All.Length }
    [string[]] Lines() { return @([Text.Encoding]::UTF8.GetString($this.All.ToArray()) -split "`n") }
    [string] Text() { return [Text.Encoding]::UTF8.GetString($this.All.ToArray()) }
    # Text from byte offset $from (cheap to poll on a large capture).
    [string] TextFrom([long]$from) {
        $from = [Math]::Max(0, [Math]::Min($from, $this.All.Length))
        return [Text.Encoding]::UTF8.GetString($this.All.GetBuffer(), [int]$from, [int]($this.All.Length - $from))
    }
    # Read until $re shows up in new output or $ms pass.
    [bool] ReadUntil([string]$re, [int]$ms) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $from = $this.All.Length
        while ($sw.ElapsedMilliseconds -lt $ms) {
            $this.ReadFor(300)
            if ($this.TextFrom($from - 200) -match $re) { return $true }
            $from = $this.All.Length
        }
        return $false
    }
    [void] Close() { try { $this.In.Close() } catch {}; if (-not $this.Proc.WaitForExit(3000)) { try { $this.Proc.Kill() } catch {} } }
}

function Get-ServerPid {
    $p = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -like "*server -s s1 -L $ns*" } | Select-Object -First 1
    if ($p) { return [int]$p.ProcessId } else { return $null }
}
function Get-PaneId {
    $id = (& $PSMUX -L $ns display-message -p -t s1 '#{pane_id}' 2>$null | Out-String).Trim()
    return $id
}
function Reset-Session {
    & $PSMUX -L $ns kill-session -t s1 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX -L $ns new-session -d -s s1 "cmd.exe" 2>&1 | Out-Null
    for ($i = 0; $i -lt 50; $i++) { & $PSMUX -L $ns has-session -t s1 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 800
}
function Count-Match([string[]]$lines, [string]$re) { return @($lines | Where-Object { $_ -match $re }).Count }

try {
    & $PSMUX -L $ns new-session -d -s s1 "cmd.exe" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1500
    $serverPid = Get-ServerPid
    $pane = Get-PaneId
    Write-Host "namespace $ns server pid $serverPid pane $pane"
    if (-not $pane) { Write-Fail "session did not start"; throw "no session" }
    $paneRe = [regex]::Escape($pane)

    # ---------------------------------------------------------------
    Write-Host "`n=== 1. stalled client with pause-after=1 is paused, backlog dropped ==="
    $cc = [CcClient]::new($PSMUX, $ns, 's1')
    $cc.ReadFor(1200)
    $cc.Send("refresh-client -f 'pause-after=1'")
    $cc.Send("refresh-client -C '100x30'")
    $cc.ReadFor(800)
    $cc.Send("send-keys -t s1 -l 'type $big'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.ReadFor(300)
    $memStart = if ($serverPid) { (Get-Process -Id $serverPid).PrivateMemorySize64 } else { 0 }
    $memMax = $memStart
    for ($i = 0; $i -lt 16; $i++) {
        Start-Sleep -Milliseconds 500
        if ($serverPid) { $m = (Get-Process -Id $serverPid -EA SilentlyContinue).PrivateMemorySize64; if ($m -gt $memMax) { $memMax = $m } }
    }
    $atResume = $cc.Bytes()
    $cc.ReadFor(3000)
    $burst = $cc.Bytes() - $atResume
    $lines = $cc.Lines()
    $pauseIdx = -1
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match "^%pause $paneRe\s*$") { $pauseIdx = $i; break } }
    $growMB = [Math]::Round(($memMax - $memStart) / 1MB, 1)
    Write-Host "  burst after resume: $burst bytes, server private growth during stall: $growMB MB"
    Check ($pauseIdx -ge 0) "%pause $pane arrives for the stalled client"
    Check ($burst -lt 4MB) "backlog delivered on resume is bounded ($burst bytes, was ~16 MB)"
    Check ($growMB -lt 6) "server memory does not grow with the backlog ($growMB MB)"
    if ($pauseIdx -ge 0) {
        $after = @($lines[($pauseIdx + 1)..($lines.Count - 1)] | Where-Object { $_ -match "^%(extended-)?output $paneRe " }).Count
        Check ($after -eq 0) "no output for the paused pane after %pause ($after lines)"
    } else { Write-Fail "no output after %pause (no %pause)" }
    Check ((Count-Match $lines '^%extended-output ') -gt 0 -and (Count-Match $lines '^%output ') -eq 0) "pause-after client gets %extended-output only"

    # Still paused while the pane keeps printing.
    $mark = $cc.Bytes()
    $cc.ReadFor(1500)
    $tail = [Text.Encoding]::UTF8.GetString($cc.All.ToArray(), [int]$mark, [int]($cc.Bytes() - $mark))
    Check ($tail -notmatch "%(extended-)?output $paneRe ") "paused pane stays silent until continue"

    Write-Host "`n=== 2. refresh-client -A '${pane}:continue' resumes the pane ==="
    # Stop the flood first so the resumed output is the marker.
    $cc.Send("send-keys -t s1 C-c")
    $cc.ReadFor(1500)
    $mark = $cc.Bytes()
    $cc.Send("refresh-client -A '${pane}:continue'")
    $cc.ReadFor(500)
    # cmd expands %M% when it reads the line, so set M on its own line first;
    # the typed text never contains the expanded marker.
    $cc.Send("send-keys -t s1 -l 'set M=OK826'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.Send("send-keys -t s1 -l 'echo %M%_DONE'")
    $cc.Send("send-keys -t s1 Enter")
    [void]$cc.ReadUntil('OK826_DONE', 5000)
    $tail = $cc.TextFrom($mark)
    Check ($tail -match "(?m)^%continue $paneRe\s*$") "%continue $pane is written"
    Check ($tail -match "%extended-output $paneRe \d+ : .*OK826_DONE") "pane output flows again after continue"
    $cc.Close()

    # ---------------------------------------------------------------
    Write-Host "`n=== 3. a client that keeps reading is never paused and gets everything ==="
    Reset-Session
    $pane = Get-PaneId; $paneRe = [regex]::Escape($pane)
    $cc = [CcClient]::new($PSMUX, $ns, 's1')
    $cc.ReadFor(1200)
    $cc.Send("refresh-client -f 'pause-after=1'")
    $cc.Send("refresh-client -C '100x30'")
    $cc.ReadFor(800)
    $cc.Send("send-keys -t s1 -l 'set M=OK826'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.ReadFor(300)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $cc.Send("send-keys -t s1 -l 'type $big & echo %M%_FAST'")
    $cc.Send("send-keys -t s1 Enter")
    $gotFast = $cc.ReadUntil('OK826_FAST', 90000)
    $lines = $cc.Lines()
    Write-Host "  fast client: $($cc.Bytes()) bytes in $($sw.ElapsedMilliseconds) ms"
    Check ((Count-Match $lines '^%pause ') -eq 0) "no %pause for a client that keeps up"
    Check $gotFast "the output after the flood arrives"
    $ages = @($lines | Where-Object { $_ -match '^%extended-output \S+ (\d+) : ' } | ForEach-Object { [int]([regex]::Match($_, '^%extended-output \S+ (\d+)').Groups[1].Value) })
    $maxAge = ($ages | Measure-Object -Maximum).Maximum
    Check ($maxAge -lt 1000) "extended-output ages stay under pause-after (max $maxAge ms)"
    $cc.Close()

    # ---------------------------------------------------------------
    Write-Host "`n=== 4. without pause-after a short stall loses nothing and is not paused ==="
    Reset-Session
    $pane = Get-PaneId; $paneRe = [regex]::Escape($pane)
    $cc = [CcClient]::new($PSMUX, $ns, 's1')
    $cc.ReadFor(1200)
    $cc.Send("refresh-client -C '100x30'")
    $cc.ReadFor(500)
    $cc.Send("send-keys -t s1 -l 'set M=OK826'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.ReadFor(300)
    $cc.Send("send-keys -t s1 -l 'type $big & echo %M%_SLOW'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.ReadFor(300)
    Start-Sleep -Seconds 4
    $gotSlow = $cc.ReadUntil('OK826_SLOW', 90000)
    $lines = $cc.Lines()
    Check ((Count-Match $lines '^%pause ') -eq 0) "no %pause without pause-after"
    Check ((Count-Match $lines '^%exit') -eq 0) "client is not dropped for a 4 s stall"
    Check ((Count-Match $lines '^%output ') -gt 0 -and (Count-Match $lines '^%extended-output ') -eq 0) "plain %output without pause-after"
    Check $gotSlow "the output after the flood arrives"

    Write-Host "`n=== 5. refresh-client -A '%N:pause' pauses on request ==="
    $mark = $cc.Bytes()
    $cc.Send("refresh-client -A '${pane}:pause'")
    $cc.ReadFor(500)
    $reply = [Text.Encoding]::UTF8.GetString($cc.All.ToArray(), [int]$mark, [int]($cc.Bytes() - $mark))
    # tmux writes the %pause inside the command's block (control_pause_pane);
    # after the %end, a client that resumes at once reads it as a new pause.
    Check ($reply -match "(?m)^%begin [^\r\n]*\r?\n%pause $paneRe\r?\n%end ") "%pause $pane is inside the command's %begin/%end"
    $cc.Send("send-keys -t s1 -l 'echo %M%_HIDDEN'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.ReadFor(1200)
    $tail = [Text.Encoding]::UTF8.GetString($cc.All.ToArray(), [int]$mark, [int]($cc.Bytes() - $mark))
    Check ($tail -match "(?m)^%pause $paneRe\s*$") "%pause $pane on request"
    Check ($tail -notmatch 'OK826_HIDDEN') "paused pane output is not sent"
    $mark2 = $cc.Bytes()
    $cc.Send("refresh-client -A '${pane}:continue'")
    $cc.ReadFor(500)
    $reply = [Text.Encoding]::UTF8.GetString($cc.All.ToArray(), [int]$mark2, [int]($cc.Bytes() - $mark2))
    Check ($reply -match "(?m)^%begin [^\r\n]*\r?\n%continue $paneRe\r?\n%end ") "%continue $pane is inside the command's %begin/%end"
    $cc.Send("send-keys -t s1 -l 'echo %M%_BACK'")
    $cc.Send("send-keys -t s1 Enter")
    $cc.ReadFor(1500)
    $tail = [Text.Encoding]::UTF8.GetString($cc.All.ToArray(), [int]$mark, [int]($cc.Bytes() - $mark))
    Check ($tail -match "(?m)^%continue $paneRe\s*$") "%continue $pane"
    Check ($tail -match 'OK826_BACK') "output resumes after continue"
    $cc.Close()
}
finally {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -like "*-L $ns*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
exit $script:TestsFailed
