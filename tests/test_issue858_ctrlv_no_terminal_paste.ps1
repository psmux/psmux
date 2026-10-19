# Issue #858: with the terminal's Ctrl+V paste binding removed, Ctrl+V in psmux
# still pasted the clipboard.
#
# parkovski, Windows Terminal with its ctrl+v action removed:
#   * `bind -n C-v split-window -h`: the window split AND the clipboard was
#     pasted into the new pane.
#   * no binding, nvim in a pane: nvim never got C-v (visual block mode), it
#     got the clipboard instead.
#
# tanaeakihiko traced it (discussion #859): the client turned the Ctrl+V
# RELEASE, with Ctrl still held, into a clipboard read and a send-paste, on the
# assumption that the terminal took the press and pasted on its own.  When the
# terminal does not paste, the press reaches the client too, nothing is pasted,
# and the release pasted the clipboard anyway.  An unbound press was swallowed
# as "the start of a paste" and never sent to the pane.
#
# tmux has no client side clipboard paste at all: C-v is 0x16 to the pane
# unless something is bound to it.  The rule now is: a press that reached the
# client with nothing paste shaped after it before the release means the
# terminal did not paste, so the release reads nothing, and an unbound press
# goes to the pane as C-v.  A press followed by key record characters is still
# one bracketed paste, and a release with no press (the route a terminal's own
# Ctrl+V binding leaves) still reads the clipboard.
#
# The host is emulated at the attached client's console input buffer with
# tests\paste_host_injector.cs, the pane child is tests\paste_recorder.cs (a vt
# byte stream reader that announces bracketed paste, like nvim).
#
# Safety: private PSMUX_DATA_DIR, its own -L namespace, kill-server only with -L,
# the clipboard is saved and restored, the client window is closed by PID.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue858_ctrlv_no_terminal_paste.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i858ctrlv_$PID"
$S = "cv"
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$savedDebug   = $env:PSMUX_INPUT_DEBUG

# Keep this path short: a long PSMUX_DATA_DIR has been seen to stop the server
# from coming up at all on this box.
$root = Join-Path $env:TEMP "p858_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "d"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_INPUT_DEBUG = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$inj = Join-Path $root "paste_host_injector.exe"
$rec = Join-Path $root "paste_recorder.exe"
& $csc /nologo /optimize /out:$inj (Join-Path $PSScriptRoot "paste_host_injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /platform:x64 /out:$rec (Join-Path $PSScriptRoot "paste_recorder.cs") 2>&1 | Out-Null
if (-not (Test-Path $inj) -or -not (Test-Path $rec)) { Write-Host "FATAL: could not compile the injector or the recorder" -ForegroundColor Red; exit 1 }

function P { & $PSMUX -L $NS @args 2>&1 }
$ESC = [char]27

$savedClip = $null
$clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}

Write-Host "`n=== Issue #858: Ctrl+V with no terminal paste ($PSMUX) ===" -ForegroundColor Cyan
$client = $null
try {
    $marker = "CLIP858x" + (Get-Random -Minimum 10000 -Maximum 99999)
    Set-Clipboard -Value $marker
    if ((Get-Clipboard -Raw) -ne $marker) { Write-Skip "the desktop refuses clipboard access to this process"; throw "noclip" }

    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 20000) {
        if (((P capture-pane -t $S -p) -join "`n") -match 'PS [A-Z]:\\|>') { break }
        Start-Sleep -Milliseconds 150
    }
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach-session", "-t", $S -PassThru -WindowStyle Normal
    $sw.Restart()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if (((P display-message -t $S -p '#{session_attached}') -join '').Trim() -eq '1') { break }
        Start-Sleep -Milliseconds 100
    }
    if (((P display-message -t $S -p '#{session_attached}') -join '').Trim() -ne '1') { Write-Fail "the attached client never registered"; throw "noclient" }
    Write-Info "attached client pid=$($client.Id)"
    Start-Sleep -Seconds 1

    $script:n = 0
    # Opens a recorder window, drives the host, ends the recorder, returns its log.
    function Record([string]$mode, [int]$delay, [string]$text) {
        $script:n++
        $log  = Join-Path $root "rec$($script:n).log"
        $stop = Join-Path $root "stop$($script:n)"
        P new-window -t $S "$rec $log 30 vt $stop" | Out-Null
        $w = [Diagnostics.Stopwatch]::StartNew()
        while ($w.ElapsedMilliseconds -lt 10000) {
            if (((P capture-pane -t $S -p) -join "`n") -match 'paste_recorder vt') { break }
            Start-Sleep -Milliseconds 100
        }
        Start-Sleep -Milliseconds 400
        & $inj $client.Id $mode $delay $text | Out-Null
        $rc = $LASTEXITCODE
        Start-Sleep -Milliseconds 1200
        New-Item -ItemType File -Force $stop | Out-Null
        $w.Restart()
        while ($w.ElapsedMilliseconds -lt 5000 -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 100 }
        Start-Sleep -Milliseconds 300
        $hex = ""
        if (Test-Path $log) { $hex = ((Get-Content $log | Where-Object { $_ -like 'HEX *' }) -replace '^HEX ', '') -join '' }
        return @{ rc = $rc; hex = $hex.Trim() }
    }
    # The recorder's window closes when it exits; wait for that before the next case.
    function Wait-OneWindow {
        $w = [Diagnostics.Stopwatch]::StartNew()
        while ($w.ElapsedMilliseconds -lt 5000) {
            if (((P display-message -t $S -p '#{session_windows}') -join '').Trim() -eq '1') { return }
            Start-Sleep -Milliseconds 100
        }
    }
    function HexOf([string]$s) { ([Text.Encoding]::UTF8.GetBytes($s) | ForEach-Object { $_.ToString("x2") }) -join "" }
    $markerHex = HexOf $marker
    $bracketed = (HexOf "$ESC[200~") + $markerHex + (HexOf "$ESC[201~")

    # --- 1. bound C-v, terminal does not paste -------------------------------
    Write-Host "`n[1] bind -n C-v split-window -h, Ctrl+V press and release, nothing pasted" -ForegroundColor Yellow
    P bind -n C-v split-window -h | Out-Null
    $before = [int](((P display-message -t $S -p '#{window_panes}') -join '').Trim())
    & $inj $client.Id plain 200 x | Out-Null
    $rc = $LASTEXITCODE
    Start-Sleep -Milliseconds 1500
    if ($rc -eq 2) { Write-Skip "AttachConsole refused" }
    else {
        $after = [int](((P display-message -t $S -p '#{window_panes}') -join '').Trim())
        if ($after -eq $before + 1) { Write-Pass "the binding ran: $before pane(s) became $after" } else { Write-Fail "window_panes $before then $after, expected one more" }
        $hits = 0
        foreach ($pi in (P list-panes -t $S -F '#{pane_id}')) {
            $cap = (P capture-pane -t "$pi" -p) -join "`n"
            if ($cap -match [regex]::Escape($marker)) { $hits++ }
        }
        if ($hits -eq 0) { Write-Pass "the clipboard was NOT pasted into any pane" } else { Write-Fail "the clipboard was pasted into $hits pane(s)" }
    }
    P unbind -n C-v | Out-Null
    P kill-pane -a -t $S | Out-Null

    # --- 2. unbound C-v reaches the pane as 0x16 -----------------------------
    Write-Host "`n[2] no binding: Ctrl+V press and release reach the pane as C-v (0x16)" -ForegroundColor Yellow
    $r = Record plain 120 x
    if ($r.rc -eq 2) { Write-Skip "AttachConsole refused" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -eq "16") { Write-Pass "the pane got exactly 0x16" } else { Write-Fail "the pane got '$($r.hex)', expected 16" }
        if ($r.hex -notmatch $markerHex) { Write-Pass "no clipboard text reached the pane" } else { Write-Fail "the clipboard was pasted" }
    }
    Wait-OneWindow

    # --- 3. a held Ctrl+V (released after the gesture window) ----------------
    Write-Host "`n[3] no binding: Ctrl+V held 600 ms, still one C-v and no paste" -ForegroundColor Yellow
    $r = Record plain 600 x
    if ($r.rc -eq 2) { Write-Skip "AttachConsole refused" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -eq "16") { Write-Pass "the pane got exactly one 0x16" } else { Write-Fail "the pane got '$($r.hex)', expected 16" }
    }
    Wait-OneWindow

    # --- 4. press then key record characters is still a paste ---------------
    foreach ($m in 'hostv', 'dripv') {
        Write-Host "`n[4 $m] Ctrl+V press, then the clipboard as key records, then the release" -ForegroundColor Yellow
        $r = Record $m 150 $marker
        if ($r.rc -eq 2) { Write-Skip "AttachConsole refused"; continue }
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -eq $bracketed) { Write-Pass "$m : one bracketed paste of the marker, nothing else" }
        else { Write-Fail "$m : pane got '$($r.hex)', expected '$bracketed'" }
        if ($r.hex -notmatch '^16|16$') { Write-Pass "$m : no stray C-v around the paste" } else { Write-Fail "$m : a C-v leaked next to the paste" }
        Wait-OneWindow
    }

    # --- 5. release only (a terminal's own Ctrl+V binding) still pastes ------
    Write-Host "`n[5] release only, no press, no characters: the client reads the clipboard" -ForegroundColor Yellow
    $r = Record release 50 x
    if ($r.rc -eq 2) { Write-Skip "AttachConsole refused" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -eq $bracketed) { Write-Pass "release only: the clipboard went in once, bracketed" } else { Write-Fail "release only: pane got '$($r.hex)', expected '$bracketed'" }
    }
    Wait-OneWindow

    # --- 6. host style paste (no press, characters, release) unchanged -------
    Write-Host "`n[6] host paste: characters then the release, no press" -ForegroundColor Yellow
    $r = Record host 150 $marker
    if ($r.rc -eq 2) { Write-Skip "AttachConsole refused" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -eq $bracketed) { Write-Pass "host paste: one bracketed paste, no duplicate" } else { Write-Fail "host paste: pane got '$($r.hex)', expected '$bracketed'" }
    }
    Wait-OneWindow

    # --- 7. paste-detection off keeps sending C-v straight through ----------
    Write-Host "`n[7] paste-detection off: Ctrl+V is C-v, as before" -ForegroundColor Yellow
    P set -g paste-detection off | Out-Null
    Start-Sleep -Milliseconds 600
    $r = Record plain 120 x
    if ($r.rc -eq 2) { Write-Skip "AttachConsole refused" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -eq "16") { Write-Pass "paste-detection off: the pane got 0x16" } else { Write-Fail "paste-detection off: pane got '$($r.hex)'" }
    }
    P set -g paste-detection on | Out-Null
    Wait-OneWindow

    # --- TUI: the client is still alive and attached ------------------------
    $att = ((P display-message -t $S -p '#{session_attached}') -join '').Trim()
    if ($att -eq '1') { Write-Pass "the client is still attached" } else { Write-Fail "session_attached=$att" }
}
catch { if ($_.Exception.Message -notin 'noclient', 'noclip') { Write-Fail "unexpected: $_" } }
finally {
    if ($client) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
    P kill-server | Out-Null
    if ($clipSaved) { try { Set-Clipboard -Value $savedClip } catch {} }
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    $env:PSMUX_INPUT_DEBUG = $savedDebug
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:Pass  Failed: $script:Fail  Skipped: $script:Skip"
exit $script:Fail
