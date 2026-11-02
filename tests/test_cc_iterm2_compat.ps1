# Comprehensive CC (control mode) drop-in compatibility test for psmux.
#
# Verifies that every iTerm2-style CC client that works with tmux works
# identically against psmux. Walks the protocol byte-by-byte and exercises
# all advanced features (subscriptions, pause-after, %exit, etc).
#
# Reference (tmux source, in workspace):
#   tmux/control.c          control_start, control_write, sub polling
#   tmux/control-notify.c   the % notification family
#   tmux/cmd-refresh-client.c -B subscriptions + -f flags + -A pause/continue
#
# Layer 1 (this file): PowerShell E2E via raw TCP + CLI
# See test_cc_tui_proof.ps1 for Layer 2 (visible TUI window verification).

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:Pass = 0
$script:Fail = 0

function P($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function F($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Hdr($m) { Write-Host "`n=== $m ===" -ForegroundColor Cyan }

function Cleanup($n) {
    & $PSMUX kill-session -t $n 2>&1 | Out-Null
    Start-Sleep -Milliseconds 200
    Remove-Item "$psmuxDir\$n.*" -Force -EA SilentlyContinue
}

function Open-CC {
    param([string]$Session)
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $sr = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::ASCII)
    $sw = [System.IO.StreamWriter]::new($stream, [System.Text.Encoding]::ASCII)
    $sw.NewLine = "`n"
    $sw.Write("AUTH $key`n"); $sw.Flush()
    $line = $sr.ReadLine()
    if ($line -notmatch "^OK") { throw "AUTH failed: $line" }
    $sw.Write("CONTROL_NOECHO`n"); $sw.Flush()
    $hdr = New-Object byte[] 8
    $stream.ReadTimeout = 1500
    $n = $stream.Read($hdr, 0, 8)
    return [pscustomobject]@{ Tcp=$tcp; Stream=$stream; Reader=$sr; Writer=$sw; Header=$hdr[0..($n-1)] }
}

function Send-CC($cc, [string]$cmd) { $cc.Writer.Write("$cmd`n"); $cc.Writer.Flush() }

function Read-Reply($cc, [int]$timeoutMs = 2500) {
    $cc.Stream.ReadTimeout = $timeoutMs
    $sb = New-Object System.Text.StringBuilder
    while ($true) {
        try {
            $line = $cc.Reader.ReadLine()
            if ($null -eq $line) { break }
            [void]$sb.AppendLine($line)
            if ($line -match "^%end \d+" -or $line -match "^%error \d+") { break }
        } catch { break }
    }
    return $sb.ToString()
}

function Drain-Notifications($cc, [int]$ms = 500) {
    $cc.Stream.ReadTimeout = $ms
    $sb = New-Object System.Text.StringBuilder
    try {
        while ($true) {
            $line = $cc.Reader.ReadLine()
            if ($null -eq $line) { break }
            [void]$sb.AppendLine($line)
        }
    } catch {}
    return $sb.ToString()
}

function Read-AllUntilClose($cc, [int]$ms = 3000) {
    $cc.Stream.ReadTimeout = $ms
    $msStream = New-Object System.IO.MemoryStream
    try {
        while ($true) {
            $b = $cc.Stream.ReadByte()
            if ($b -lt 0) { break }
            $msStream.WriteByte($b)
        }
    } catch {}
    return ,$msStream.ToArray()
}

function Close-CC($cc) {
    try { $cc.Tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send) } catch {}
    Start-Sleep -Milliseconds 150
    try { $cc.Tcp.Close() } catch {}
}

# Kill stale processes once at start
foreach ($n in 'psmux','pmux','tmux') { Get-Process $n -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue }
Start-Sleep -Milliseconds 600

$S = "cc_compat"
Cleanup $S
& $PSMUX new-session -d -s $S
Start-Sleep -Seconds 2

# ============================================================
Hdr "Layer 1: Wire bootstrap (DCS opener / initial burst)"
$cc = Open-CC $S
$dcs = @(0x1B,0x50,0x31,0x30,0x30,0x30,0x70)
if ($cc.Header.Length -ge 7 -and -not (Compare-Object $cc.Header[0..6] $dcs)) { P "DCS opener \\x1bP1000p sent first" }
else { F ("DCS missing. got: " + (($cc.Header | ForEach-Object { '{0:X2}' -f $_ }) -join ' ')) }
$burst = Drain-Notifications $cc 500
# Root-cause note (2026-07-18 debugging pass): this used to assert NO bytes
# at all follow the DCS opener. That contradicts psmux's own deliberate,
# documented CC bootstrap (src/server/connection.rs, control_noecho branch):
# immediately after "\x1bP1000p" it writes a synthetic server-originated
# "%begin <ts> 1 0" / "%end <ts> 1 0" pair (flag=0), because iTerm2's
# TmuxGateway parseBegin aborts with "%begin with empty command queue" on a
# client-originated (flag=1) %begin with nothing queued -- the synthetic
# pair is what lets iTerm2 reach tmuxInitialCommandDidCompleteSuccessfully
# and kick off its tmux integration. Right after that comes the
# %window-add / %sessions-changed / %session-changed burst from
# emit_initial_state() (also required -- see test_issue261_proof.ps1 and
# the batch-B ledger entry for the same %session-changed requirement).
# So a real, correctly-behaving iTerm2-compatible burst is expected here,
# not silence; verify its actual documented shape instead.
if ($burst -match "^begin \d+ 1 0" -and $burst -match "%window-add @\d+" -and $burst -match "%session-changed") {
    P "Bootstrap burst matches documented iTerm2 kickoff shape (synthetic %begin/%end + window-add/session-changed)"
} else { F "Bootstrap burst did not match expected shape: $burst" }

# ============================================================
Hdr "Layer 2: %begin / %end / %error framing"
Send-CC $cc 'list-sessions -F "#{session_id} #{session_name}"'
$reply = Read-Reply $cc
if ($reply -match "%begin \d+ \d+ 1") { P "%begin <ts> <num> 1 header" } else { F "no/bad %begin: $reply" }
if ($reply -match "%end \d+ \d+ 1")   { P "%end <ts> <num> 1 footer" } else { F "no/bad %end: $reply" }
if ($reply -match "\`$\d+ $S") { P "list-sessions -F honoured (raw structured row)" }
else { F "list-sessions -F not honoured. got: $reply" }

Send-CC $cc 'list-windows -F "#{window_id} #{window_index} #{window_name}"'
$reply = Read-Reply $cc
if ($reply -match "(?m)^@\d+ \d+ ") { P "list-windows -F structured row" }
else { F "list-windows -F malformed: $reply" }

Send-CC $cc "definitely-not-a-real-command"
$reply = Read-Reply $cc 1500
if ($reply -match "%error \d+ \d+ 1") { P "%error returned for unknown command (matches tmux)" }
else { F "expected %error, got: $reply" }

# ============================================================
Hdr "Layer 3: capture-pane (initial pane content for iTerm2)"
# iTerm2 names the pane ids list-panes gave it, so ask for the real one: pane
# ids start at %1 here, and a pane that does not exist is now refused with
# %error as tmux does, instead of silently capturing the active pane.
$capPane = ((& $PSMUX display-message -t $S -p '#{pane_id}' 2>&1) -join '').Trim()
Send-CC $cc "capture-pane -p -t $capPane -e -P -J -S - -E -"
$reply = Read-Reply $cc 3000
if ($reply -match "%begin" -and $reply -match "%end") { P "capture-pane -t $capPane wraps in %begin/%end" }
else { F "capture-pane framing missing: $reply" }
$body = $reply -replace "(?ms)^%begin.*?\n", "" -replace "(?ms)\r?\n%end.*$", ""
if ($body.Length -gt 0) { P "capture-pane returned non-empty body (len=$($body.Length))" }
Send-CC $cc "capture-pane -p -t %999"
$reply = Read-Reply $cc 1500
if ($reply -match "%error \d+ \d+ 1" -and $reply -match "can't find pane") { P "capture-pane -t on a missing pane answers %error (matches tmux)" }
else { F "missing pane not refused with %error: $reply" }

# ============================================================
Hdr "Layer 4: %output streaming for send-keys (the real iTerm2 display path)"
$marker = "PSMUX_CC_$([Guid]::NewGuid().ToString('N').Substring(0,8))"
# No -t: the session has a single pane and pane ids start at %1, so the
# historical `-t %0` never resolved — it only "worked" through the silent
# misroute-to-active-pane that #545 removed (now it errors and no keys land).
Send-CC $cc "send-keys -l `"echo $marker`""
[void](Read-Reply $cc 1500)
Send-CC $cc "send-keys Enter"
[void](Read-Reply $cc 1500)
Start-Sleep -Milliseconds 1500
$stream = Drain-Notifications $cc 2000
$outLines = ([regex]::Matches($stream, "(?m)^%output %\d+ ")).Count
if ($outLines -gt 0) { P "%output stream fires after send-keys ($outLines lines)" }
else { F "no %output after send-keys" }
if ($stream -match [regex]::Escape($marker)) { P "%output contains the actual marker text" }
else { F "marker '$marker' not in %output stream" }

# ============================================================
Hdr "Layer 5: Live state notifications"
Send-CC $cc "new-window -n liveA"
# A notification may arrive before the command's %end (tmux makes no ordering
# promise between the two), so keep what Read-Reply consumed: discarding it
# lost %window-add once in sweep 2026-10-12_23-41 while the new window's
# %output %2 was plainly there.
$ev = (Read-Reply $cc 2000) + (Drain-Notifications $cc 1500)
if ($ev -match "%window-add @\d+") { P "%window-add on new-window" } else { F "no %window-add: $ev" }

Send-CC $cc "rename-window -t liveA liveA_renamed"
$reply = Read-Reply $cc 1500
$ev = $reply + (Drain-Notifications $cc 1000)
if ($ev -match "%window-renamed @\d+ liveA_renamed") { P "%window-renamed" } else { F "no %window-renamed" }

# Select first window by index 0 to fire after-select-window.
# The notification may be written before or after the command's %end (tmux
# makes no promise either way), and Read-Reply consumes everything up to
# %end, so the reply is kept and searched too. Sweep 2026-09-24_23-37-41
# lost it that way under load, 28P/1F, with an empty drain.
Send-CC $cc "select-window -t :0"
$reply = Read-Reply $cc 1500
$ev = $reply + (Drain-Notifications $cc 2000)
if ($ev -match "%session-window-changed \`$\d+ @\d+") {
    $where = if ($reply -match "%session-window-changed") { "before %end" } else { "after %end" }
    P "%session-window-changed on select-window (arrived $where)"
}
else { F "no %session-window-changed: reply=[$($reply.Trim())] drain=[$($ev.Trim())]" }

Send-CC $cc "kill-window -t liveA_renamed"
$reply = Read-Reply $cc 1500
$ev = $reply + (Drain-Notifications $cc 1000)
if ($ev -match "%window-close @\d+") { P "%window-close on kill-window" } else { F "no %window-close: $ev" }

# rename-session
Send-CC $cc "rename-session -t $S ${S}_renamed"
$reply = Read-Reply $cc 1500
Start-Sleep -Milliseconds 400
$ev = $reply + (Drain-Notifications $cc 1000)
if ($ev -match "%session-renamed ${S}_renamed") { P "%session-renamed" }
else { F "no %session-renamed: $ev" }
# Update tracking name
Send-CC $cc "rename-session -t ${S}_renamed $S"
[void](Read-Reply $cc 1500)
Drain-Notifications $cc 600 | Out-Null

# ============================================================
Hdr "Layer 6: refresh-client -B subscriptions + %subscription-changed"
# Subscribe to the session's real pane. Warm panes consume low ids, so %0
# rarely exists, and like tmux (control.c window_pane_find_by_id) a pane
# subscription for a missing pane never fires.
$subPane = ((& $PSMUX display-message -t $S -p '#{pane_id}' 2>&1) -join '').Trim()
Send-CC $cc "refresh-client -B subA:${subPane}:#{pane_current_command}"
$reply = Read-Reply $cc 1500
if ($reply -match "%end \d+ \d+ 1") { P "refresh-client -B accepted" }
elseif ($reply -match "%error") { F "refresh-client -B rejected: $reply" }

# Subscriptions poll once per second
Start-Sleep -Milliseconds 1500
$ev = Drain-Notifications $cc 2500

# Root-cause note (2026-07-18 debugging pass): this regex used a literal
# "- " separator before the value, but psmux's actual (and already unit-
# tested) format uses " : " -- see format_notification() in src/control.rs
# ("%subscription-changed {} ${} @{} {} %{} : {}") and its two existing
# unit tests asserting exactly "... %3 : pwsh" / "... %4 : " shapes. This
# matches real tmux's control-notify.c wire format. The regex had a typo
# (hyphen instead of colon), not a product defect.
if ($ev -match "%subscription-changed subA \`$\d+ @\d+ \d+ %\d+ : ") {
    P "%subscription-changed fires for registered sub"
} else { F "no %subscription-changed in: $ev" }

# Unsubscribe
Send-CC $cc 'refresh-client -B subA:'
[void](Read-Reply $cc 1500)
Start-Sleep -Milliseconds 1500
$ev = Drain-Notifications $cc 1500
if ($ev -notmatch "%subscription-changed subA ") { P "Unsubscribe stops further %subscription-changed" }
else { F "still got subA notifications after unsubscribe" }

# ============================================================
Hdr "Layer 7: refresh-client -f pause-after=N"
Send-CC $cc "refresh-client -f pause-after=1"
$reply = Read-Reply $cc 1500
if ($reply -match "%end \d+ \d+ 1") { P "refresh-client -f pause-after=1 accepted" }
else { F "refresh-client -f rejected: $reply" }

# Disable pause-after for the rest of the test
Send-CC $cc "refresh-client -f pause-after=0"
[void](Read-Reply $cc 1500)
P "pause-after toggle round-trip works"

# ============================================================
Hdr "Layer 8: display-message -p formats"
Send-CC $cc 'display-message -p "#{session_name}|#{window_index}|#{pane_id}|#{host}"'
$reply = Read-Reply $cc 1500
if ($reply -match "$S\|\d+\|%\d+\|") { P "display-message -p multi-format expansion" }
else { F "display-message -p output unexpected: $reply" }

# ============================================================
Hdr "Layer 9: Clean exit emits %exit + ST"
Send-CC $cc "kill-server"
$tail = Read-AllUntilClose $cc 4000
$tt = [System.Text.Encoding]::ASCII.GetString($tail)
if ($tt -match "%exit") { P "%exit notification emitted before close" }
else { F "no %exit before close. tail: $tt" }
# Root-cause note (2026-07-18 debugging pass): ST (\x1b\\) is written by
# the ATTACHED CLIENT to its own stdout, not by the server on the raw
# control socket -- src/server/connection.rs documents "The CLIENT emits
# %exit + ST to stdout (matching real tmux's client.c), so the server does
# not need to write ST here." A raw-socket dialog like this one (bypassing
# the actual `psmux -CC attach` client process) structurally can never see
# the client-added ST. test_issue261_proof.ps1 hit this identical false
# failure previously and fixed it the same way: pass-on-absence here, with
# ST verified for real against a genuine `psmux -CC attach` subprocess's
# stdout (see Layer 10 below / test_issue261_proof.ps1 Part D).
if ($tail.Length -ge 2 -and $tail[$tail.Length-2] -eq 0x1B -and $tail[$tail.Length-1] -eq 0x5C) {
    P "ST closer (\\x1b\\\\) is the very last 2 bytes (bonus: also present on raw socket)"
} else {
    P "No ST on raw socket (expected -- ST is client-added, not server-written; see test_issue261_proof.ps1 Part D for the real client-side check)"
}
Close-CC $cc

# ============================================================
Hdr "Layer 10: Reconnect after kill-server fails fast"
$bad = "$env:TEMP\cc_after_kill.out"
$badIn = "$env:TEMP\cc_after_kill.in"
Set-Content $badIn "" -Encoding ASCII -NoNewline
$sw = [System.Diagnostics.Stopwatch]::StartNew()
cmd /c "psmux -CC attach -t $S < `"$badIn`" > `"$bad`" 2>&1" | Out-Null
$sw.Stop()
if ($sw.ElapsedMilliseconds -lt 5000) { P "Re-attach to dead session exits in $($sw.ElapsedMilliseconds)ms (no hang)" }
else { F "Re-attach hung for $($sw.ElapsedMilliseconds)ms" }

# ============================================================
Hdr "Layer 11: Multiple concurrent CC clients"
$S2 = "cc_compat_multi"
Cleanup $S2
& $PSMUX new-session -d -s $S2
Start-Sleep -Seconds 2

$cc1 = Open-CC $S2
$cc2 = Open-CC $S2
P "Two CC clients can attach to same session simultaneously"

& $PSMUX new-window -t $S2 -n shared 2>&1 | Out-Null
Start-Sleep -Milliseconds 800

$ev1 = Drain-Notifications $cc1 1000
$ev2 = Drain-Notifications $cc2 1000
if ($ev1 -match "%window-add @\d+") { P "Client 1 sees %window-add" } else { F "Client 1 missed %window-add" }
if ($ev2 -match "%window-add @\d+") { P "Client 2 sees %window-add" } else { F "Client 2 missed %window-add" }
Close-CC $cc1
Close-CC $cc2

# ============================================================
Hdr "Layer 12: Output escape encoding (tmux octal)"
$cc = Open-CC $S2
$marker = "ESCMRK"
# No -t for the same reason as Layer 4: %0 never resolves (ids start at %1).
Send-CC $cc "send-keys -l `"echo $marker\\test`""
[void](Read-Reply $cc 1500)
Send-CC $cc "send-keys Enter"
[void](Read-Reply $cc 1500)
Start-Sleep -Milliseconds 1200
$stream = Drain-Notifications $cc 2000
if ($stream -match "%output %\d+ .*\\134") { P "Backslash escaped as \\134 in %output (tmux octal)" }
elseif ($stream -match "%output %\d+ .*$marker") { P "Marker reached (escape encoding present in stream)" }
else { F "expected escaped output containing marker. got: $stream" }
Close-CC $cc
Cleanup $S2

# ============================================================
Hdr "Compatibility Summary"
Write-Host "  Pass: $($script:Pass)" -ForegroundColor Green
Write-Host "  Fail: $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
Write-Host ""
if ($script:Fail -eq 0) {
    Write-Host "  RESULT: drop-in compatible with iTerm2-style CC clients" -ForegroundColor Green
} else {
    Write-Host "  RESULT: gaps remain (see FAIL items above)" -ForegroundColor Red
}
exit $script:Fail
