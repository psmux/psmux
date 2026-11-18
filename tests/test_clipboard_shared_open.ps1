# psmux clipboard writes and reads while another process opens the clipboard
# with no window (pwsh Get-Clipboard does).
#
# Full sweep 2026-11-16_19-30-29 failed four clipboard suites that pass alone:
# set-buffer -w warned "could not write the system clipboard: Windows refused
# the text" and left the old text (test_clipboard_write_busy), a paste put
# nothing in the pane (test_issue428_busy_clipboard_paste) and refresh-client
# -l stored nothing (test_refresh_client_l_server_route).
#
# psmux opened the clipboard with OpenClipboard(NULL), which does not lock out
# another process that also opens it with no window: both get it, and the
# first CloseClipboard closes it for both. psmux's EmptyClipboard then failed
# (the warning) or its GetClipboardData returned nothing (the lost paste and
# refresh). Master with a Get-Clipboard loop running: 67 of 200 set-buffer -w
# refused, 21 of 100 refresh-client -l lost. psmux now opens the clipboard
# with its own message-only window, so the open is exclusive.
#
# Safety: private PSMUX_DATA_DIR, its own -L namespace, kill-server only with
# -L, the Get-Clipboard loop is stopped by PID, the user's clipboard is saved
# and restored.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_clipboard_shared_open.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$root = Join-Path $env:TEMP "pcso_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "d"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$NS = "clipshared_$PID"
function P { & $PSMUX -L $NS @args 2>&1 }

. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")

$savedClip = $null; $clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}
$opener = $null

Write-Host "`n=== clipboard writes and reads beside a no-window opener ($PSMUX) ===" -ForegroundColor Cyan
try {
    if (-not (Set-ClipboardVerified "CSOPROBE")) { Write-Skip "the desktop refuses clipboard access to this process"; throw "noclip" }
    P new-session -d -s c -x 80 -y 24 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) { P has-session -t c | Out-Null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 150 }

    # Another process reading the clipboard as fast as it can.
    $opener = Start-Process pwsh -ArgumentList '-NoProfile', '-Command', '$e=[DateTime]::Now.AddMinutes(5); while([DateTime]::Now -lt $e){ $null = Get-Clipboard -Raw }' -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2

    Write-Host "`n[1] set-buffer -w, 60 times" -ForegroundColor Yellow
    $refused = 0; $silent = 0; $ok = 0; $firstBad = ''
    for ($i = 0; $i -lt 60; $i++) {
        $want = "CSO_W$i"
        $err = (& $PSMUX -L $NS set-buffer -w $want 2>&1 | Out-String).Trim()
        if ($err) { $refused++; if (-not $firstBad) { $firstBad = $err }; continue }
        $clip = Get-ClipboardSettled
        if ($clip -eq $want) { $ok++ } else { $silent++; if (-not $firstBad) { $firstBad = "no warning, clipboard [$clip]" } }
    }
    if ($refused -eq 0 -and $silent -eq 0) { Write-Pass "60 of 60 written, no warning" }
    else { Write-Fail "$ok written, $refused warned, $silent silently not written; first: $firstBad" }

    Write-Host "`n[2] refresh-client -l from a hook, 30 times" -ForegroundColor Yellow
    [void](Set-ClipboardVerified "CSO_READ")
    P set-hook -g after-new-window "refresh-client -l" | Out-Null
    $lost = 0; $got = 0
    for ($i = 0; $i -lt 30; $i++) {
        P set-buffer "CSO_OLD$i" | Out-Null
        P new-window -d | Out-Null
        $top = ''
        $w = [Diagnostics.Stopwatch]::StartNew()
        while ($w.ElapsedMilliseconds -lt 3000) { $top = (P show-buffer) -join ''; if ($top -eq 'CSO_READ') { break }; Start-Sleep -Milliseconds 30 }
        if ($top -eq 'CSO_READ') { $got++ } else { $lost++ }
        if ($i % 10 -eq 9) { P kill-window -a -t c:0 | Out-Null }
    }
    if ($lost -eq 0) { Write-Pass "30 of 30 stored the clipboard" }
    else { Write-Fail "$lost of 30 stored nothing (top buffer kept the old one), $got stored it" }
}
catch { if ($_.Exception.Message -ne 'noclip') { Write-Fail "unexpected: $_" } }
finally {
    if ($opener -and -not $opener.HasExited) { Stop-Process -Id $opener.Id -Force -EA SilentlyContinue }
    P kill-server | Out-Null
    if ($clipSaved -and $null -ne $savedClip) { if (-not (Set-ClipboardVerified $savedClip)) { Write-Host "  [WARN] the clipboard could not be restored" -ForegroundColor Yellow } }
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:Pass  Failed: $script:Fail  Skipped: $script:Skip"
exit $script:Fail
