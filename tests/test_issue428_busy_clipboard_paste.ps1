# #428: prefix + ] and paste-buffer with an empty paste stack paste the OS
# clipboard. A window holding the clipboard open for longer than ~10 ms (Radeon
# Software, a VNC server, a clipboard history listener) made the paste vanish:
# with a 100 ms holder, 0 of 5 pastes landed on master 18b7b633 (2026-11-12
# sweep: test_issue428_proof F2 "prefix+] did not paste clipboard").
#
# The read now waits off the server loop (up to 1 s) and the server holds every
# later request behind it: keys typed right after prefix + ] land AFTER the
# pasted text, and a clipboard held past the wait loses the paste but never the
# keys.
#
# Safety: private PSMUX_DATA_DIR, own -L namespace, kill-server only with -L,
# the attached window it opens is closed by PID, holders cap at 5 s, the
# clipboard is saved and restored.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue428_busy_clipboard_paste.ps1
$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$root = Join-Path $env:TEMP "p428busy_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force "$root\d" | Out-Null
$env:PSMUX_DATA_DIR = "$root\d"
$NS = "i428busy_$PID"
function P { & $PSMUX -L $NS @args 2>&1 }

$injector = "$env:TEMP\psmux_injector.exe"
if (-not (Test-Path $injector)) {
    & "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe" /nologo /optimize "/out:$injector" (Join-Path $PSScriptRoot "injector.cs") | Out-Null
}
$holder = "$root\clipboard_holder.exe"
& "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe" -nologo -r:System.Windows.Forms.dll "-out:$holder" (Join-Path $PSScriptRoot "clipboard_holder.cs") | Out-Null
if (-not (Test-Path $holder) -or -not (Test-Path $injector)) { Write-Host "FATAL: could not build the holder or the injector" -ForegroundColor Red; exit 1 }

$script:holders = @()
function Hold([int]$ms) {
    $ready = "$root\ready_$([guid]::NewGuid().ToString('N').Substring(0,6))"
    $hp = Start-Process $holder -ArgumentList $ms, $ready -PassThru -WindowStyle Hidden
    $script:holders += $hp
    $w = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path $ready) -and $w.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 2 }
    if (-not (Test-Path $ready)) { return $null }
    return $hp
}

$savedClip = $null; $clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}
$proc = $null
Write-Host "`n=== #428 paste of a busy clipboard ($PSMUX) ===" -ForegroundColor Cyan
try {
    $proc = Start-Process -FilePath $PSMUX -ArgumentList '-L', $NS, 'new-session', '-s', 's' -PassThru
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) { P has-session -t s | Out-Null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 200 }
    Start-Sleep -Seconds 2

    # One attempt: empty the stack, seed the clipboard, hold it, paste, look.
    function Attempt([string]$Marker, [int]$HoldMs, [string]$How) {
        for ($i = 0; $i -lt 25; $i++) { P delete-buffer | Out-Null }
        if (-not (Set-ClipboardVerified $Marker)) { return $null }
        P send-keys -t s C-c | Out-Null
        P send-keys -t s 'cls' Enter | Out-Null
        Start-Sleep -Milliseconds 700
        $hp = if ($HoldMs -gt 0) { Hold $HoldMs } else { $null }
        if ($HoldMs -gt 0 -and -not $hp) { return $null }
        switch ($How) {
            'key'     { & $injector $proc.Id "^b{SLEEP:400}]" | Out-Null }
            'keytail' { & $injector $proc.Id "^b{SLEEP:400}]ZQTAIL" | Out-Null }
            'cli'     { P paste-buffer -t s | Out-Null }
        }
        if ($hp) { $hp.WaitForExit(7000) | Out-Null }
        Start-Sleep -Milliseconds 1000
        return ((P capture-pane -t s -p) | Out-String)
    }

    $N = 3
    $cases = @(
        @{ name = 'free clipboard, prefix + ]';                          hold = 0;   how = 'key';     want = { param($c, $m) $c -match $m } },
        @{ name = 'clipboard held 100 ms while paste-buffer reads it';     hold = 100; how = 'cli';     want = { param($c, $m) $c -match $m } },
        @{ name = 'clipboard held 600 ms while prefix + ] reads it';       hold = 600; how = 'key';     want = { param($c, $m) $c -match $m } },
        @{ name = 'keys typed right after prefix + ] land after the paste'; hold = 600; how = 'keytail'; want = { param($c, $m) $c -match "${m}ZQTAIL" } },
        @{ name = 'held 3000 ms: the paste gives up, the typed keys are kept'; hold = 3000; how = 'keytail'; want = { param($c, $m) ($c -match 'ZQTAIL') -and ($c -notmatch $m) } }
    )
    $k = 1
    foreach ($case in $cases) {
        Write-Host "`n[$k] $($case.name)" -ForegroundColor Yellow
        $k++
        $ok = 0; $ran = 0; $last = ''
        for ($i = 0; $i -lt $N; $i++) {
            $m = "M428B_$($k)_$i"
            $c = Attempt $m $case.hold $case.how
            if ($null -eq $c) { continue }
            $ran++
            if (& $case.want $c $m) { $ok++ } else { $last = (($c -split "`n" | Where-Object { $_ -match '\S' } | Select-Object -Last 2) -join ' | ') }
        }
        if ($ran -eq 0) { Write-Skip "the clipboard could not be seeded or held" }
        elseif ($ok -eq $ran) { Write-Pass "$ok/$ran" }
        else { Write-Fail "$ok/$ran; last pane tail: $last" }
    }
}
catch { Write-Fail "unexpected: $_" }
finally {
    foreach ($h in $script:holders) { if ($h -and -not $h.HasExited) { Stop-Process -Id $h.Id -Force -EA SilentlyContinue } }
    P kill-server | Out-Null
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    if ($clipSaved) { [void](Set-ClipboardVerified $savedClip) }
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:Pass  Failed: $script:Fail  Skipped: $script:Skip"
exit $script:Fail
