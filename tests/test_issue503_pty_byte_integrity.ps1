# Issue #503: "Reader thread silently drops/corrupts bytes from the child's PTY
# under sustained streaming output (data loss, not a display bug)"
#
# The report claimed that characters go missing between the child process and
# psmux's reader thread, evidenced by stripping ANSI from a `pipe-pane -o`
# capture of an LLM streaming Thai text and finding codepoints absent.
#
# This suite tests that claim with a deterministic generator instead of an LLM
# stream, and it separates two things the report conflated:
#
#   A. Does psmux lose or corrupt characters under sustained streaming?
#      (Parts 1-4.) Answer must be no: every record must survive byte-exact.
#
#   B. Is the raw PTY byte stream a transcript of the child's writes on Windows?
#      (Part 5.) Answer is no, and that is the platform, not psmux. ConPTY is a
#      terminal emulator that renders on a timer and coalesces frames the child
#      wrote but that were never displayed. Part 5 proves this happens
#      identically with psmux removed from the path entirely, so flattening a
#      pipe-pane capture and diffing it against the child's writes cannot show
#      data loss. This is the flaw in the report's methodology.
#
# tmux parity: tmux tees raw pty data to pipe-pane before parsing
# (window.c, bufferevent_write(wp->pipe_event, new_data, new_size)), which is
# exactly what psmux does. The behaviour is at parity; only the platform differs,
# because a Unix pty is a byte passthrough and ConPTY is not.

$ErrorActionPreference = "Continue"
# psmux writes UTF-8, and PowerShell decodes a native command's output with
# [Console]::OutputEncoding, the console code page (OEM 437 in the runner's
# window), so the non-ASCII text this suite compares must be read as UTF-8.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$SESSION = "issue503"
$SESSION_TUI = "issue503_tui"
$work = "$env:TEMP\psmux503_suite"

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

# The exact Thai word the reporter saw corrupted ("ปัจจุบัน" -> "ปัจจุณั").
$REAL  = [char]0x0E1B+[char]0x0E31+[char]0x0E08+[char]0x0E08+[char]0x0E38+[char]0x0E1A+[char]0x0E31+[char]0x0E19+
         [char]0x0E2A+[char]0x0E34+[char]0x0E49+[char]0x0E19+[char]0x0E40+[char]0x0E14+[char]0x0E37+[char]0x0E2D+[char]0x0E19
$DECOY = [char]0x0E17+[char]0x0E31+[char]0x0E22+[char]0x0E22+[char]0x0E38+[char]0x0E17+[char]0x0E31+[char]0x0E22+
         [char]0x0E17+[char]0x0E34+[char]0x0E49+[char]0x0E22+[char]0x0E40+[char]0x0E17+[char]0x0E37+[char]0x0E22+[char]0x0E17

function Normalize($t) {
    # Strip OSC, CSI and other escapes, then line breaks, so that terminal line
    # wrapping does not split a record and read as data loss.
    $t = [regex]::Replace($t, "\x1b\][^\x07\x1b]*(\x07|\x1b\\)", "")
    $t = [regex]::Replace($t, "\x1b\[[0-9;?]*[ -/]*[@-~]", "")
    $t = [regex]::Replace($t, "\x1b[@-Z\\-_]", "")
    return ($t -replace "[\r\n]", "")
}

function Get-Records($text) {
    return [regex]::Matches($text, '<S(\d{6})>(.*?)</S\1>')
}

function Cleanup {
    foreach ($s in @($SESSION, $SESSION_TUI)) {
        & $PSMUX kill-session -t $s 2>&1 | Out-Null
    }
    Get-Process rawsink -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Start-Sleep -Milliseconds 600
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
    Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue
}

# === BUILD HELPERS ===
Write-Host "`n=== Issue #503: PTY byte integrity under sustained streaming ===" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $work | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$testsDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$GEN  = "$work\streamgen.exe"
$SINK = "$work\rawsink.exe"
$CAP  = "$work\conptycap.exe"
& $csc /nologo /optimize /out:$GEN  "$testsDir\streamgen.cs"  2>&1 | Out-Null
& $csc /nologo /optimize /out:$SINK "$testsDir\rawsink.cs"    2>&1 | Out-Null
& $csc /nologo /optimize /out:$CAP  "$testsDir\conptycap.cs"  2>&1 | Out-Null
if ((Test-Path $GEN) -and (Test-Path $SINK) -and (Test-Path $CAP)) {
    Write-Pass "Test helpers compiled (streamgen, rawsink, conptycap)"
} else {
    Write-Fail "Could not compile test helpers; aborting"
    exit 1
}

# === PART 0: ground truth ===
Write-Host "`n[Part 0] Ground truth: what the child actually writes" -ForegroundColor Yellow
$baseBin = "$work\baseline.bin"
Start-Process -FilePath $GEN -ArgumentList "200","7","0","thai","0" `
    -RedirectStandardOutput $baseBin -RedirectStandardError "$work\baseline.err" -NoNewWindow -Wait
$baseText = [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($baseBin))
$baseRecs = Get-Records $baseText
$baseBad = @($baseRecs | Where-Object { $_.Groups[2].Value -ne $REAL }).Count
if ($baseRecs.Count -eq 200 -and $baseBad -eq 0) {
    Write-Pass "Generator emits 200/200 byte-exact records with no terminal in the path"
} else {
    Write-Fail "Generator itself is unsound: records=$($baseRecs.Count)/200 corrupt=$baseBad"
}

# === SESSION SETUP ===
Cleanup
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session creation failed; aborting"; exit 1 }
& $PSMUX set-option -t $SESSION -g history-limit 20000 2>&1 | Out-Null
Write-Pass "Session '$SESSION' created ($((& $PSMUX display-message -t $SESSION -p '#{pane_width}x#{pane_height}' 2>&1)))"

# Runs the generator in the pane with pipe-pane capturing the raw PTY read,
# and returns both the raw capture and the resulting pane grid.
function Invoke-StreamRun {
    param([int]$Records, [int]$Chunk, [string]$Mode = "thai")

    $raw = "$work\run_${Records}_${Chunk}_${Mode}.bin"
    if (Test-Path $raw) { [System.IO.File]::Delete($raw) }

    & $PSMUX pipe-pane -o -t $SESSION "$SINK $raw" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    & $PSMUX send-keys -t $SESSION "& '$GEN' $Records $Chunk 0 $Mode 0" Enter 2>&1 | Out-Null

    $last = -1; $stable = 0
    for ($i = 0; $i -lt 600; $i++) {
        Start-Sleep -Milliseconds 500
        $len = 0
        try { $len = (Get-Item $raw -EA Stop).Length } catch {}
        if ($len -eq $last -and $len -gt 0) { $stable++ } else { $stable = 0 }
        $last = $len
        if ($stable -ge 6) { break }
    }

    $grid = & $PSMUX capture-pane -t $SESSION -p -S - 2>&1 | Out-String
    & $PSMUX pipe-pane -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Get-Process rawsink -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Start-Sleep -Milliseconds 400

    $bytes = [System.IO.File]::ReadAllBytes($raw)
    return @{ Bytes = $bytes; Raw = (Normalize ([System.Text.Encoding]::UTF8.GetString($bytes))); Grid = (Normalize $grid) }
}

function Assert-NoLoss {
    param([string]$Label, $Result, [int]$Records)

    $recs = Get-Records $Result.Raw
    $seen = @{}; $corrupt = @()
    foreach ($m in $recs) {
        $seen[[int]$m.Groups[1].Value] = $true
        if ($m.Groups[2].Value -ne $REAL) { $corrupt += ("#" + $m.Groups[1].Value + "=[" + $m.Groups[2].Value + "]") }
    }
    $missing = @(); for ($i = 0; $i -lt $Records; $i++) { if (-not $seen.ContainsKey($i)) { $missing += $i } }

    $strict = [System.Text.UTF8Encoding]::new($false, $true)
    $utf8ok = $true
    try { $null = $strict.GetString($Result.Bytes) } catch { $utf8ok = $false }

    Write-Info "$Label -> $($Result.Bytes.Length) bytes captured, $($seen.Count)/$Records distinct records"
    if ($missing.Count -eq 0) { Write-Pass "$Label : no records lost ($Records/$Records present)" }
    else { Write-Fail "$Label : $($missing.Count) records LOST, first: $(($missing | Select-Object -First 15) -join ',')" }

    if ($corrupt.Count -eq 0) { Write-Pass "$Label : every payload byte-exact" }
    else { Write-Fail "$Label : $($corrupt.Count) payloads CORRUPT, first: $($corrupt[0])" }

    if ($utf8ok) { Write-Pass "$Label : capture is valid UTF-8 (no split multi-byte sequences)" }
    else { Write-Fail "$Label : capture contains invalid UTF-8" }
}

# === PART 1: sustained streaming, the reported scenario ===
Write-Host "`n[Part 1] Sustained streaming, small writes (the reported scenario)" -ForegroundColor Yellow
$r1 = Invoke-StreamRun -Records 200 -Chunk 7
Assert-NoLoss "200 records / 7-byte writes" $r1 200

# === PART 2: worst case, every multi-byte sequence split across writes ===
Write-Host "`n[Part 2] Byte-at-a-time writes (splits every UTF-8 sequence)" -ForegroundColor Yellow
$r2 = Invoke-StreamRun -Records 800 -Chunk 1
Assert-NoLoss "800 records / 1-byte writes" $r2 800

# === PART 3: the reporter's data volume (418KB) ===
Write-Host "`n[Part 3] Reporter's capture volume (~430KB)" -ForegroundColor Yellow
$r3 = Invoke-StreamRun -Records 6000 -Chunk 3
Assert-NoLoss "6000 records / 3-byte writes" $r3 6000

# === PART 4: what the user actually sees ===
Write-Host "`n[Part 4] Pane grid content (what the user actually sees)" -ForegroundColor Yellow
$gridRecs = Get-Records $r3.Grid
$gridSeen = @{}; $gridBad = 0
foreach ($m in $gridRecs) {
    $gridSeen[[int]$m.Groups[1].Value] = $true
    if ($m.Groups[2].Value -ne $REAL) { $gridBad++ }
}
Write-Info "grid holds $($gridSeen.Count) distinct records (bounded by history-limit)"
if ($gridSeen.Count -ge 1000) { Write-Pass "Pane grid retained a large record set from the stream" }
else { Write-Fail "Pane grid retained only $($gridSeen.Count) records" }
if ($gridBad -eq 0) { Write-Pass "Every record rendered into the grid is byte-exact" }
else { Write-Fail "$gridBad records in the grid have corrupt payloads" }

# === PART 5: the report's methodology, and why it cannot show data loss ===
Write-Host "`n[Part 5] ConPTY coalesces frames: raw stream is not a write transcript" -ForegroundColor Yellow
Write-Info "Child writes each record twice: a decoy frame, CR, then the real frame."
Write-Info "The decoy is never displayed. If the raw stream were a transcript of the"
Write-Info "child's writes, every decoy would appear in it."

# 5a: literal transcript, no terminal involved
$repaintBase = "$work\repaint_baseline.bin"
Start-Process -FilePath $GEN -ArgumentList "500","7","0","repaint","0" `
    -RedirectStandardOutput $repaintBase -RedirectStandardError "$work\repaint.err" -NoNewWindow -Wait
$rbText = Normalize ([System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($repaintBase)))
$baseDecoys = ([regex]::Matches($rbText, [regex]::Escape($DECOY))).Count
if ($baseDecoys -eq 500) { Write-Pass "Child genuinely writes 500 decoy frames (no terminal in path)" }
else { Write-Fail "Expected 500 decoy frames from the child, got $baseDecoys" }

# 5b: through psmux
$r5 = Invoke-StreamRun -Records 500 -Chunk 7 -Mode "repaint"
$psmuxDecoys = ([regex]::Matches($r5.Raw, [regex]::Escape($DECOY))).Count
$psmuxGridDecoys = ([regex]::Matches($r5.Grid, [regex]::Escape($DECOY))).Count
$r5Recs = Get-Records $r5.Raw
$r5Seen = @{}; foreach ($m in $r5Recs) { $r5Seen[[int]$m.Groups[1].Value] = $true }
Write-Info "through psmux: $psmuxDecoys of 500 decoy frames survived into the raw stream"
if ($psmuxDecoys -lt 50) {
    Write-Pass "ConPTY discarded $((500 - $psmuxDecoys)) of 500 frames the child wrote, before psmux could read them"
} else {
    Write-Fail "Expected heavy frame coalescing, but $psmuxDecoys decoys survived"
}
if ($r5Seen.Count -eq 500) { Write-Pass "All 500 records still present despite frame coalescing" }
else { Write-Fail "Only $($r5Seen.Count)/500 records present" }
if ($psmuxGridDecoys -eq 0) { Write-Pass "No decoy frame is visible in the pane grid (display is correct)" }
else { Write-Fail "$psmuxGridDecoys decoy frames leaked into the visible grid" }

# 5c: the control, psmux removed from the path entirely
$env:CONPTYCAP_DRAIN_MS = "60000"
$ctrlBin = "$work\conpty_control.bin"
& $CAP $ctrlBin 120 30 0 $GEN 500 7 0 repaint 3000 2>&1 | Out-Null
if (Test-Path $ctrlBin) {
    $ctrlText = Normalize ([System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($ctrlBin)))
    $ctrlDecoys = ([regex]::Matches($ctrlText, [regex]::Escape($DECOY))).Count
    $ctrlSeen = @{}; foreach ($m in (Get-Records $ctrlText)) { $ctrlSeen[[int]$m.Groups[1].Value] = $true }
    Write-Info "bare ConPTY, no psmux: $ctrlDecoys of 500 decoy frames survived, $($ctrlSeen.Count)/500 records present"
    # ConPTY's frame coalescing is nondeterministic run to run: which one or
    # two of the 500 decoys happen to survive is timing noise, so an exact
    # `ctrl <= psmux` comparison flips on single-frame jitter (observed 1 vs 2
    # and 0 vs 1 across consecutive runs of the same binaries). The regression
    # this arm exists to catch is psmux EATING frames wholesale, which shows
    # up as a difference far above jitter, so allow a small tolerance.
    if ($ctrlDecoys -le ($psmuxDecoys + 3)) {
        Write-Pass "psmux keeps frame survival within jitter of bare ConPTY (psmux=$psmuxDecoys, bare=$ctrlDecoys): the loss is the platform's"
    } else {
        Write-Fail "psmux discarded far more frames ($psmuxDecoys survived) than bare ConPTY ($ctrlDecoys survived)"
    }
} else {
    Write-Fail "ConPTY control harness produced no capture"
}

# === PART 6: TCP server path ===
Write-Host "`n[Part 6] TCP server command path" -ForegroundColor Yellow
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key  = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
try {
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $auth = $reader.ReadLine()
    if ($auth -eq "OK") {
        Write-Pass "TCP AUTH accepted"
        $marker = "TCPMARK_" + $REAL
        $writer.Write("send-keys -t $SESSION `"echo $marker`" Enter`n"); $writer.Flush()
        $stream.ReadTimeout = 10000
        try { $null = $reader.ReadLine() } catch {}
        Start-Sleep -Seconds 2
        $capTcp = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($capTcp -match ("TCPMARK_" + [regex]::Escape($REAL))) {
            Write-Pass "Thai text sent over the TCP path renders byte-exact in the pane"
        } else {
            Write-Fail "Thai marker sent over TCP did not render correctly"
        }
    } else {
        Write-Fail "TCP AUTH failed: $auth"
    }
    $tcp.Close()
} catch {
    Write-Fail "TCP path error: $($_.Exception.Message)"
}

# === PART 7: Win32 TUI visual verification ===
Write-Host "`n[Part 7] Win32 TUI visual verification (real attached window)" -ForegroundColor Yellow
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 5
& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "TUI: attached session is live"

    & $PSMUX send-keys -t $SESSION_TUI "& '$GEN' 300 5 0 thai 0" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 12
    $tuiGrid = Normalize (& $PSMUX capture-pane -t $SESSION_TUI -p -S - 2>&1 | Out-String)
    $tuiRecs = Get-Records $tuiGrid
    $tuiBad = @($tuiRecs | Where-Object { $_.Groups[2].Value -ne $REAL }).Count
    if ($tuiRecs.Count -gt 0 -and $tuiBad -eq 0) {
        Write-Pass "TUI: $($tuiRecs.Count) streamed records rendered byte-exact in a real window"
    } else {
        Write-Fail "TUI: $tuiBad of $($tuiRecs.Count) rendered records are corrupt"
    }

    & $PSMUX split-window -v -t $SESSION_TUI 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1200
    $panes = (& $PSMUX display-message -t $SESSION_TUI -p '#{window_panes}' 2>&1).Trim()
    if ($panes -eq "2") { Write-Pass "TUI: split-window produced 2 panes (renderer still healthy)" }
    else { Write-Fail "TUI: expected 2 panes, got $panes" }
} else {
    Write-Fail "TUI: attached session did not start"
}
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# === TEARDOWN ===
Cleanup
Remove-Item $work -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
