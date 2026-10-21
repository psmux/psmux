# A paste on a Turkish Q layout lost every `i` and was not one paste.
#
# Windows Terminal pastes by writing the clipboard into the client's console
# input, and the console turns each character into the key that types it on the
# active layout.  On Turkish Q that key for `i` is AltGr+I, so the record for a
# pasted `i` carries LEFT_CTRL | RIGHT_ALT (measured on Windows 11 26200):
#
#   DOWN vk=0x12 scan=0x38 uChar=0x0000 ctrl=0x0109   (right Alt)
#   DOWN vk=0x49 scan=0x17 uChar=0x0069 ctrl=0x0009
#
# The client took Ctrl+Alt on a lowercase ASCII letter for a real chord: the
# pending paste went out as typing at every `i` and the pane got C-M-i, a Tab.
# A pane holding Claude Code showed "b       r sonrak" for "bir sonraki".
#
# tests\paste_host_injector.cs `wttr` writes the records Windows Terminal wrote
# for each character on that layout, hard coded, so this test does not depend
# on the keyboard layout of the machine it runs on.  The pane child is
# tests\paste_recorder.cs (a vt byte stream reader that announces bracketed
# paste, like Claude Code and nvim).
#
# Safety: private PSMUX_DATA_DIR, its own -L namespace, kill-server only with -L
# and checked to have taken the server down, the clipboard is saved and
# restored, the client window is closed by PID.  The
# default session list (`psmux ls`, read only) is taken before and after (AGENTS.md);
# if the names differ the test fails and asks for an investigation.  It never
# touches the default namespace: a user creating or renaming a session during the
# run trips it too.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_altgr_letter_paste.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "altgri_$PID"
$S = "ag"
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

# Names of the sessions in the DEFAULT namespace and data dir (read only, no -L).
# Window counts and the attached flag are left out: a user's own session changes
# those on its own, a session appearing or going away is what this watches for.
# stdout carries the names (`ls -F`), stderr only the error, so an error line
# is never read as a session. "no server running" / "no sessions" is an empty
# list; any other failure is reported as unreadable, not as an empty list.
function Get-DefaultSessions {
    $keep = @{}
    foreach ($v in 'PSMUX_DATA_DIR','PSMUX_NO_WARM','PSMUX_INPUT_DEBUG','PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
        $keep[$v] = [Environment]::GetEnvironmentVariable($v)
        Remove-Item "Env:\$v" -EA SilentlyContinue
    }
    $errFile = [IO.Path]::GetTempFileName()
    try {
        $names = @(& $PSMUX ls -F '#{session_name}' 2>$errFile | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
        $err = (Get-Content $errFile -Raw -EA SilentlyContinue)
        if ($code -eq 0) { return @{ ok = $true; names = @($names | Sort-Object -CaseSensitive) } }
        if ("$err" -cmatch 'no server running|no sessions') { return @{ ok = $true; names = @() } }
        return @{ ok = $false; names = @(); err = "exit $code $err".Trim() }
    }
    finally {
        Remove-Item $errFile -EA SilentlyContinue
        foreach ($v in $keep.Keys) { if ($null -ne $keep[$v]) { Set-Item "Env:\$v" $keep[$v] } }
    }
}
$defaultBefore = Get-DefaultSessions
if ($defaultBefore.ok) { Write-Info ("default sessions before: " + $defaultBefore.names.Count) }
else { Write-Info "default session list unreadable before the run: $($defaultBefore.err)" }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$savedDebug   = $env:PSMUX_INPUT_DEBUG

$root = Join-Path $env:TEMP "pagi_$PID"
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
function HexOf([string]$s) { ([Text.Encoding]::UTF8.GetBytes($s) | ForEach-Object { $_.ToString("x2") }) -join "" }

# What a node raw mode reader with bracketed paste on received from the same
# Ctrl+V in Windows Terminal with no psmux in between (26200, Turkish Q):
# one paste, the CRLF of the clipboard as CR.
$textA = "bir sonraki Eski Insight Beti" + [char]0x011F + "in`r" + [char]0x0130 + "kinci sat" + [char]0x0131 + "r: " + [char]0x015F + "i" + [char]0x015F + "e " + [char]0x0131 + [char]0x011F + "d" + [char]0x0131 + "r @ son"
$measuredA = "1b5b3230307e62697220736f6e72616b692045736b6920496e73696768742042657469c49f696e0dc4b06b696e636920736174c4b1723a20c59f69c59f6520c4b1c49f64c4b172204020736f6e1b5b3230317e"
$fileA = Join-Path $root "a.txt"
[IO.File]::WriteAllText($fileA, $textA, [Text.UTF8Encoding]::new($false))

$savedClip = $null
$clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}

Write-Host "`n=== AltGr letter paste on Turkish Q ($PSMUX) ===" -ForegroundColor Cyan
$client = $null
try {
    # The clipboard holds the text being pasted, as it does for a real paste,
    # because the client recognises the head of a paste by the clipboard's
    # first characters.  A tail that is only on the clipboard follows it: a
    # path that reads the clipboard instead of the injected records would put
    # that tail into the pane, and the byte comparisons below would fail.
    $clipTail = " [clipboard only $(Get-Random -Minimum 10000 -Maximum 99999)]"
    Set-Clipboard -Value ($textA + $clipTail)

    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 20000) {
        if (((P capture-pane -t $S -p) -join "`n") -cmatch 'PS [A-Z]:\\|>') { break }
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
    function Record([string]$mode, [int]$delay, [string]$text) {
        $script:n++
        $log  = Join-Path $root "rec$($script:n).log"
        $stop = Join-Path $root "stop$($script:n)"
        P new-window -t $S "$rec $log 30 vt $stop" | Out-Null
        $w = [Diagnostics.Stopwatch]::StartNew()
        while ($w.ElapsedMilliseconds -lt 10000) {
            if (((P capture-pane -t $S -p) -join "`n") -cmatch 'paste_recorder vt') { break }
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
    function Wait-OneWindow {
        $w = [Diagnostics.Stopwatch]::StartNew()
        while ($w.ElapsedMilliseconds -lt 5000) {
            if (((P display-message -t $S -p '#{session_windows}') -join '').Trim() -eq '1') { return }
            Start-Sleep -Milliseconds 100
        }
    }

    # --- 1. the measured paste ----------------------------------------------
    Write-Host "`n[1] Windows Terminal paste on Turkish Q: i as AltGr+I, CR, Turkish letters, @" -ForegroundColor Yellow
    $expected = (HexOf "$ESC[200~") + (HexOf $textA) + (HexOf "$ESC[201~")
    if ($expected -ceq $measuredA) { Write-Pass "the expected bytes are the ones measured without psmux" } else { Write-Fail "test text does not match the measurement" }
    $r = Record wttr 150 "@$fileA"
    if ($r.rc -ne 0) { Write-Fail "the injector failed (exit $($r.rc))" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -ceq $measuredA) { Write-Pass "one bracketed paste, byte for byte what the pane gets without psmux" }
        else { Write-Fail "pane got '$($r.hex)', expected '$measuredA'" }
        if ($r.hex -cnotmatch '^(..)*09') { Write-Pass "no Tab reached the pane" } else { Write-Fail "a Tab (C-M-i) reached the pane" }
    }
    Wait-OneWindow

    # --- 2. a text that opens and closes on i --------------------------------
    Write-Host "`n[2] Turkish Q paste that starts and ends with i" -ForegroundColor Yellow
    $textB = "iyi bir gun dilerim i"
    Set-Clipboard -Value ($textB + $clipTail)
    $bracketedB = (HexOf "$ESC[200~") + (HexOf $textB) + (HexOf "$ESC[201~")
    $r = Record wttr 150 $textB
    if ($r.rc -ne 0) { Write-Fail "the injector failed (exit $($r.rc))" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -ceq $bracketedB) { Write-Pass "one bracketed paste of '$textB'" } else { Write-Fail "pane got '$($r.hex)', expected '$bracketedB'" }
    }
    Wait-OneWindow

    # --- 2b. doubled letters -------------------------------------------------
    # Measured: every pasted `i` is its own record with wRepeatCount 1, wrapped
    # in its own AltGr down and up, even four in a row; nothing is coalesced.
    Write-Host "`n[2b] Turkish Q paste with doubled i: siir fiil tabii iiii aa" -ForegroundColor Yellow
    $textC = [string][char]0x015F + "iir fiil tabii iiii aa"
    Set-Clipboard -Value ($textC + $clipTail)
    $bracketedC = (HexOf "$ESC[200~") + (HexOf $textC) + (HexOf "$ESC[201~")
    $r = Record wttr 150 $textC
    if ($r.rc -ne 0) { Write-Fail "the injector failed (exit $($r.rc))" }
    else {
        Write-Info "pane received: $($r.hex)"
        if ($r.hex -ceq $bracketedC) { Write-Pass "one bracketed paste, every doubled letter kept" } else { Write-Fail "pane got '$($r.hex)', expected '$bracketedC'" }
    }
    Wait-OneWindow

    # --- 3. Left Ctrl + Left Alt + I is still C-M-i ---------------------------
    Write-Host "`n[3] Left Ctrl + Left Alt + I (ctrl=0x000A, i in uChar) is still C-M-i" -ForegroundColor Yellow
    # Built by hand, not measured: a physical chord was measured with uChar 0,
    # and crossterm gives such a record a letter through the foreground window's
    # layout, which a test cannot pin.  With `i` in uChar crossterm reads
    # Char('i') CONTROL|ALT on every layout, and it is not the AltGr signature
    # (LEFT_CTRL | RIGHT_ALT), so the client must send C-M-i.  The pane byte 09
    # would not tell (a plain Tab gives it too); the client's input debug log
    # names the event and the command.  No log, no event or a failed injection
    # is a failure, never a skip.
    $dbgLog = Join-Path $env:PSMUX_DATA_DIR "input_debug.log"
    function Log-Lines { if (Test-Path $dbgLog) { @(Get-Content $dbgLog -EA SilentlyContinue) } else { @() } }
    $mark = (Log-Lines).Count
    $r = Record ctrlalti 0 x
    if ($r.rc -ne 0) { Write-Fail "the injector failed (exit $($r.rc))" }
    elseif (-not (Test-Path $dbgLog)) { Write-Fail "no client input debug log at $dbgLog" }
    else {
        $new = @((Log-Lines) | Select-Object -Skip $mark)
        $ev = @($new | Where-Object { $_ -cmatch '\[event\] Key ' })
        $sawChord = @($ev | Where-Object { $_ -cmatch "code=Char\('i'\) mods=KeyModifiers\(CONTROL \| ALT\) kind=Press" })
        $sent = @($new | Where-Object { $_ -cmatch '\[send\].*send-key C-M-i$' })
        $asText = @($new | Where-Object { $_ -cmatch '\[send\].*send-(text|paste)' })
        Write-Info ("client events: " + (($ev | ForEach-Object { ($_ -replace '^.*\[event\] ', '') }) -join ' ; '))
        if ($sawChord.Count -ge 1) { Write-Pass "crossterm read Char('i') with CONTROL | ALT" } else { Write-Fail "no Ctrl+Alt+i press event in the client log" }
        if ($sent.Count -ge 1) { Write-Pass "the client sent send-key C-M-i for the chord" } else { Write-Fail "no send-key C-M-i in the client log after the chord" }
        if ($asText.Count -eq 0) { Write-Pass "the chord was not sent as text" } else { Write-Fail "the chord went out as text: $($asText -join ' | ')" }
    }
    Wait-OneWindow

    $att = ((P display-message -t $S -p '#{session_attached}') -join '').Trim()
    if ($att -eq '1') { Write-Pass "the client is still attached" } else { Write-Fail "session_attached=$att" }
}
catch { if ($_.Exception.Message -ne 'noclient') { Write-Fail "unexpected: $_" } }
finally {
    if ($client) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
    P kill-server | Out-Null
    # The test namespace must be gone: `ls` on it reports no server.
    $nsGone = $false
    $w = [Diagnostics.Stopwatch]::StartNew()
    while ($w.ElapsedMilliseconds -lt 5000) {
        $o = (& $PSMUX -L $NS ls 2>&1 | ForEach-Object { "$_" }) -join ' '
        if ($LASTEXITCODE -ne 0 -and $o -cmatch 'no server running') { $nsGone = $true; break }
        Start-Sleep -Milliseconds 200
    }
    if ($nsGone) { Write-Pass "the -L $NS server is gone after cleanup" } else { Write-Fail "the -L $NS server still answers after kill-server: $o" }
    if ($clipSaved) { try { Set-Clipboard -Value $savedClip } catch {} }
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    $env:PSMUX_INPUT_DEBUG = $savedDebug
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue

    # Read only. A difference is reported for investigation; nothing in the
    # default namespace is touched, whoever changed it.
    $defaultAfter = Get-DefaultSessions
    Write-Host "`n=== default session list ===" -ForegroundColor Cyan
    if (-not $defaultBefore.ok -or -not $defaultAfter.ok) {
        Write-Fail "could not read the default session list (before: $($defaultBefore.err) after: $($defaultAfter.err)); investigation needed"
    }
    elseif (($defaultBefore.names -join "`n") -ceq ($defaultAfter.names -join "`n")) { Write-Pass "the default session list is the same before and after ($($defaultAfter.names.Count) session(s))" }
    else { Write-Fail "the default session list changed during the run ($($defaultBefore.names.Count) before, $($defaultAfter.names.Count) after); investigation needed, the test did not touch it" }
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:Pass  Failed: $script:Fail  Skipped: $script:Skip"
exit $script:Fail
