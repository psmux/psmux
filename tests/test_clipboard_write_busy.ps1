# set-buffer -w / load-buffer -w with a clipboard another window holds open.
#
# Full sweeps 2026-11-11 and 2026-11-12 failed test_issue298_set_buffer_clipboard
# ("set-buffer -w with spaces in content: got 'SENTINEL'", "load-buffer -w got
# ''"). The command line wrote the clipboard with about 10 ms of retries and
# then gave up without a word, so a clipboard history listener, a VNC or RDP
# clipboard bridge or a phone link app holding it open a little longer
# silently left the old clipboard in place.
#
# Now the command line write waits for a busy clipboard up to 1 s, and warns
# on stderr (exit 0, as tmux cannot know whether its OSC 52 landed either)
# when it still could not write.
#
# Safety: private PSMUX_DATA_DIR, its own -L namespace, kill-server only with
# -L, the user's clipboard is saved and restored, holders cap themselves at
# 5 s and are stopped by PID.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_clipboard_write_busy.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$root = Join-Path $env:TEMP "pcwb_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "d"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$NS = "clipwbusy_$PID"
function P { & $PSMUX -L $NS @args 2>&1 }

$holder = Join-Path $root "clipboard_holder.exe"
& "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe" "-nologo" "-r:System.Windows.Forms.dll" "-out:$holder" (Join-Path $PSScriptRoot "clipboard_holder.cs") 2>&1 | Out-Null
if (-not (Test-Path $holder)) { Write-Host "FATAL: could not compile clipboard_holder.cs" -ForegroundColor Red; exit 1 }

. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")

$script:holders = @()
function Start-Holder([int]$HoldMs) {
    $ready = Join-Path $root "ready_${HoldMs}_$([guid]::NewGuid().ToString('N').Substring(0,6))"
    $hp = Start-Process $holder -ArgumentList $HoldMs, $ready -PassThru -WindowStyle Hidden
    $script:holders += $hp
    $w = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path $ready) -and $w.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 2 }
    if (-not (Test-Path $ready)) { return $null }
    return $hp
}

# Runs one psmux command line with the clipboard held for $HoldMs.
function Run-Held([int]$HoldMs, [string[]]$CmdArgs) {
    $hp = Start-Holder $HoldMs
    if (-not $hp) { return $null }
    $errFile = Join-Path $root "err_$([guid]::NewGuid().ToString('N').Substring(0,6)).txt"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS @CmdArgs 2> $errFile | Out-Null
    $code = $LASTEXITCODE
    $ms = $sw.ElapsedMilliseconds
    $hp.WaitForExit(7000) | Out-Null
    $err = (Get-Content $errFile -Raw -EA SilentlyContinue)
    if ($null -eq $err) { $err = '' }
    return @{ code = $code; ms = $ms; err = $err.Trim(); clip = (Get-ClipboardSettled) }
}

$savedClip = $null; $clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}

Write-Host "`n=== set-buffer -w / load-buffer -w with a busy clipboard ($PSMUX) ===" -ForegroundColor Cyan
try {
    if (-not (Set-ClipboardVerified "CWBPROBE")) { Write-Skip "the desktop refuses clipboard access to this process"; throw "noclip" }
    P new-session -d -s w -x 80 -y 24 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000 -and ((P has-session -t w) -match 'no server|can.t find')) { Start-Sleep -Milliseconds 150 }

    Write-Host "`n[1] free clipboard: set-buffer -w writes it, no warning" -ForegroundColor Yellow
    [void](Set-ClipboardVerified "CWB_OLD1")
    $errFile = Join-Path $root "err_free.txt"
    & $PSMUX -L $NS set-buffer -w CWB_FREE 2> $errFile | Out-Null
    $code = $LASTEXITCODE
    $err = (Get-Content $errFile -Raw -EA SilentlyContinue)
    $clip = Get-ClipboardSettled
    if ($clip -eq 'CWB_FREE' -and $code -eq 0 -and -not $err) { Write-Pass "clipboard [CWB_FREE], exit 0, stderr empty" }
    else { Write-Fail "clipboard [$clip] exit $code stderr [$err]" }

    $cases = @(
        @{ hold = 100; name = 'set-buffer -w';  args = @('set-buffer', '-w', 'CWB_SET100'); want = 'CWB_SET100' },
        @{ hold = 300; name = 'set-buffer -w';  args = @('set-buffer', '-w', 'CWB_SET300'); want = 'CWB_SET300' },
        @{ hold = 100; name = 'load-buffer -w'; file = 'CWB_LOAD100'; want = 'CWB_LOAD100' },
        @{ hold = 300; name = 'load-buffer -w'; file = 'CWB_LOAD300'; want = 'CWB_LOAD300' }
    )
    $n = 2
    foreach ($c in $cases) {
        Write-Host "`n[$n] clipboard held $($c.hold) ms: $($c.name) waits and writes it" -ForegroundColor Yellow
        $n++
        [void](Set-ClipboardVerified "CWB_OLD")
        $a = $c.args
        if ($c.file) {
            $f = Join-Path $root "$($c.file).txt"
            [IO.File]::WriteAllText($f, $c.file)
            $a = @('load-buffer', '-w', $f)
        }
        $r = Run-Held $c.hold $a
        if ($null -eq $r) { Write-Skip "the holder could not take the clipboard"; continue }
        $top = (P show-buffer) -join ''
        if ($r.clip -eq $c.want -and $r.code -eq 0 -and -not $r.err -and $top -eq $c.want) {
            Write-Pass "clipboard [$($r.clip)] after $($r.ms) ms, buffer [$top], no warning"
        } else {
            Write-Fail "clipboard [$($r.clip)] (want $($c.want)) after $($r.ms) ms, exit $($r.code), stderr [$($r.err)], buffer [$top]"
        }
    }

    Write-Host "`n[$n] clipboard held 3000 ms: set-buffer -w gives up after about 1 s, warns, still sets the buffer" -ForegroundColor Yellow
    [void](Set-ClipboardVerified "CWB_OLD_LONG")
    $r = Run-Held 3000 @('set-buffer', '-w', 'CWB_LONG')
    if ($null -eq $r) { Write-Skip "the holder could not take the clipboard" }
    else {
        $top = (P show-buffer) -join ''
        $okWarn = $r.err -match 'could not write the system clipboard'
        $okTime = $r.ms -ge 900 -and $r.ms -lt 2800
        if ($okWarn -and $okTime -and $r.code -eq 0 -and $top -eq 'CWB_LONG' -and $r.clip -eq 'CWB_OLD_LONG') {
            Write-Pass "warned [$($r.err)] after $($r.ms) ms, exit 0, buffer [$top], clipboard kept [$($r.clip)]"
        } else {
            Write-Fail "stderr [$($r.err)] after $($r.ms) ms, exit $($r.code), buffer [$top], clipboard [$($r.clip)]"
        }
    }
}
catch { if ($_.Exception.Message -ne 'noclip') { Write-Fail "unexpected: $_" } }
finally {
    foreach ($h in $script:holders) { if ($h -and -not $h.HasExited) { Stop-Process -Id $h.Id -Force -EA SilentlyContinue } }
    P kill-server | Out-Null
    if ($clipSaved) {
        $back = $false
        for ($i = 0; $i -lt 50 -and -not $back; $i++) {
            try { Set-Clipboard -Value $savedClip -EA Stop } catch {}
            $back = ($(try { Get-Clipboard -Raw -EA Stop } catch { $null }) -eq $savedClip)
            if (-not $back) { Start-Sleep -Milliseconds 100 }
        }
        if (-not $back) { Write-Host "  [WARN] the clipboard could not be restored" -ForegroundColor Yellow }
    }
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:Pass  Failed: $script:Fail  Skipped: $script:Skip"
exit $script:Fail
