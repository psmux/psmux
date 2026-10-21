# An application's OSC 52 clipboard write and bell must reach the attached
# client's terminal even when the frame that carried them is superseded.
#
# Frames are full snapshots, so the server's per client frame slot, the
# client's line queue (#882, client_frame_queue.rs) and the client's drain all
# keep only the newest unread frame. The clipboard write and the bell rode
# inside a frame as one shot fields and went with it: measured on 0145958f,
# 20 back to back OSC 52 writes from a pane reached the client's terminal 0
# times, a client whose output was stalled for 6 s got none either, and the
# Windows clipboard was never set. tmux writes both to the client's tty as
# they happen (input.c:3415-3440, screen-write.c:3014-3024, alerts.c:314).
#
# The client runs attached with PSMUX_PIPE_VT=1, so its terminal output is a
# pipe this script reads (and stops reading, to stall it).
#
# Isolation: private PSMUX_DATA_DIR, unique -L namespace, cleanup only with
# `-L <ns> kill-server`, only processes this script started are stopped. The
# user's clipboard is saved first and restored in `finally`.
# PSMUX_TEST_EXE picks the binary. Exit code is the fail count.

$ErrorActionPreference = 'Stop'
$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
$ns = "oneshot_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_oneshot_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$env:PSMUX_DATA_DIR = $data
Remove-Item Env:\SHELL -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX_PANE -ErrorAction SilentlyContinue

$script:pass = 0; $script:fail = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:pass++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>$null }

$savedClip = $null
$haveClip = $false
try { $savedClip = Get-Clipboard -Raw; $haveClip = $true } catch {}

$script:client = $null
function Start-Client([string]$Session) {
    $psi = [Diagnostics.ProcessStartInfo]::new($exe)
    foreach ($a in @('-L', $ns, 'attach', '-t', $Session)) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.Environment['PSMUX_PIPE_VT'] = '1'; $psi.Environment['TERM'] = 'xterm-256color'
    $script:client = [Diagnostics.Process]::Start($psi)
    $script:client.StandardInput.Write("$([char]27)[8;50;160t"); $script:client.StandardInput.Flush()
    $script:out = $script:client.StandardOutput.BaseStream
    $script:sink = New-Object IO.MemoryStream
    $script:buf = New-Object byte[] 65536
    $script:readTask = $null
}
# Read the client's terminal output for $Ms milliseconds.
function Pump([int]$Ms) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $Ms) {
        if ($null -eq $script:readTask) { $script:readTask = $script:out.ReadAsync($script:buf, 0, $script:buf.Length) }
        if ($script:readTask.Wait(100)) {
            $n = $script:readTask.Result; $script:readTask = $null
            if ($n -le 0) { break }
            $script:sink.Write($script:buf, 0, $n)
        }
    }
}
function Stop-Client {
    if ($script:client -and -not $script:client.HasExited) { try { $script:client.Kill() } catch {} ; $script:client.WaitForExit(3000) | Out-Null }
    $script:client = $null
}
function Marks([string]$Text) {
    @([regex]::Matches($Text, "\x1b\]52;c;([A-Za-z0-9+/=]*)") | ForEach-Object {
        [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($_.Groups[1].Value)) })
}
# The pane writes $Count OSC 52 writes (OSCMARK1..N, ST terminated) and,
# with -Bell, one BEL; with -Flood each write follows 400 lines of output.
function Emit([string]$Session, [int]$Count, [switch]$Flood, [switch]$Bell) {
    $fl = if ($Flood) { "1..400 | % { Write-Output ('fill ' + `$_ + ' ' + ('y' * 100)) }; " } else { "" }
    $bl = if ($Bell) { "[Console]::Write([char]7); " } else { "" }
    $cmd = "1..$Count | % { $fl[Console]::Write([char]27 + ']52;c;' + [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('OSCMARK' + `$_)) + [char]27 + '\') }; $bl'EMIT' + 'DONE'"
    P send-keys -t $Session $cmd Enter
}
function New-Pane([string]$Session) {
    P new-session -d -s $Session -x 160 -y 50
    for ($i = 0; $i -lt 50 -and -not ((P capture-pane -p -t $Session) -join '').Trim(); $i++) { Start-Sleep -Milliseconds 200 }
    P set -t $Session -g set-clipboard on
    P set -t $Session -g bell-action any
    P set -t $Session -g visual-bell off
}
function Wait-Done([string]$Session) {
    for ($i = 0; $i -lt 100 -and -not (((P capture-pane -p -t $Session) -join "`n") -match 'EMITDONE'); $i++) { Start-Sleep -Milliseconds 200 }
}
function Set-Sentinel { if ($haveClip) { Set-Clipboard -Value "ONESHOT_SENTINEL" } }
function Clip { if ($haveClip) { (Get-Clipboard -Raw) } else { "" } }

try {
    Write-Host "`n=== one shot clipboard and bell delivery ===" -ForegroundColor Cyan
    Write-Host "  Binary: $exe  Namespace: $ns"

    Write-Host "`n[1] one OSC 52 from the pane" -ForegroundColor Yellow
    New-Pane 'a'; Set-Sentinel
    Start-Client 'a'; Pump 2500
    Emit 'a' 1; Wait-Done 'a'; Pump 2500
    $text = [Text.Encoding]::UTF8.GetString($script:sink.ToArray())
    $m = @(Marks $text)
    Check 'reaches the client terminal' (($m -join ',') -eq 'OSCMARK1') "marks: [$($m -join ',')]"
    if ($haveClip) { Check 'sets the Windows clipboard' ((Clip) -eq 'OSCMARK1') "clipboard: [$(Clip)]" }
    Stop-Client

    Write-Host "`n[2] 20 OSC 52 writes back to back: the last one reaches the terminal" -ForegroundColor Yellow
    New-Pane 'b'; Set-Sentinel
    Start-Client 'b'; Pump 2500
    Emit 'b' 20; Wait-Done 'b'; Pump 2500
    $m = @(Marks ([Text.Encoding]::UTF8.GetString($script:sink.ToArray())))
    Check 'the newest write arrives last' ($m.Count -ge 1 -and $m[-1] -eq 'OSCMARK20') "marks: [$($m -join ',')]"
    Check 'no write arrives twice' (($m | Group-Object | Where-Object Count -gt 1).Count -eq 0) "marks: [$($m -join ',')]"
    if ($haveClip) { Check 'clipboard holds the newest write' ((Clip) -eq 'OSCMARK20') "clipboard: [$(Clip)]" }
    Stop-Client

    Write-Host "`n[3] client output stalled for 6 s while the pane floods, writes and rings" -ForegroundColor Yellow
    New-Pane 'c'; Set-Sentinel
    Start-Client 'c'; Pump 2500
    $before = $script:sink.Length
    Emit 'c' 20 -Flood -Bell
    Start-Sleep -Seconds 6
    Wait-Done 'c'
    Pump 4000
    $all = $script:sink.ToArray()
    $text = [Text.Encoding]::UTF8.GetString($all, [int]$before, [int]($all.Length - $before))
    $m = @(Marks $text)
    Check 'the newest write reaches the terminal after the stall' ($m.Count -ge 1 -and $m[-1] -eq 'OSCMARK20') "marks: [$($m -join ',')]"
    # A BEL that is not an OSC terminator (the client ends OSC 52 with BEL).
    $bells = [regex]::Matches(($text -replace "\x1b\][^\x07\x1b]*(\x07|\x1b\\)", ''), "\x07").Count
    Check 'the bell reaches the terminal after the stall' ($bells -ge 1) "standalone BEL: $bells"
    if ($haveClip) { Check 'clipboard holds the newest write' ((Clip) -eq 'OSCMARK20') "clipboard: [$(Clip)]" }
    Check 'the client is still attached' (-not $script:client.HasExited -and @(P list-clients -t c | Where-Object { $_ }).Count -ge 1) ''
    Stop-Client

    Write-Host "`n[4] the server still reports the write to a dump-state poll" -ForegroundColor Yellow
    New-Pane 'd'
    Emit 'd' 1; Wait-Done 'd'; Start-Sleep -Milliseconds 300
    $dump = (P dump-state -t d) -join ''
    Check 'dump-state carries clipboard_osc52 (wire format unchanged)' ($dump -match '"clipboard_osc52":"T1NDTUFSSzE="') ''
}
catch { Check "unexpected error" $false "$_" }
finally {
    Stop-Client
    P kill-server | Out-Null
    if ($haveClip) { try { Set-Clipboard -Value $savedClip } catch {} }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}
Write-Host "`n  Passed: $script:pass  Failed: $script:fail"
exit $script:fail
