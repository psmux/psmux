# Issue #1049: a held key must never reach the pane as a paste.
#
# Over ssh, holding `j` in Neovim's normal mode now and then stopped moving the
# cursor and typed `jjjjjjjj` into the buffer. The keyboard repeats a held key
# evenly, 30 ms or so apart, but the network hands several repeats over at once
# whenever it stalls, three or more inside the client's 20 ms window, and the
# client took that for a paste:
#
#   [paste] stage2: 4 chars in 20ms, waiting for Ctrl+V Release
#   [paste] stage2 paste ended by a key after 133 ms quiet, sending 4 chars as send-paste before it
#   child:  ESC[200~ jjjj ESC[201~
#
# Neovim inserts a bracketed paste whatever mode it is in.
#
# The shape is emulated with tests/paste_host_injector.cs `clumps`: four `j` in
# one WriteConsoleInputW, a clump every 133 ms, no Ctrl anywhere.
#
# The second case is the guard: a pasted run of one character (a `----------`
# rule) is still a paste, because the clipboard holds it.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1049_held_key_not_a_paste.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "i1049held" }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$savedClip    = try { Get-Clipboard -Raw -EA SilentlyContinue } catch { $null }

$root = Join-Path $env:TEMP "psmux_i1049_held"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$recorder = Join-Path $root "paste_recorder.exe"
$inj = Join-Path $root "paste_host_injector.exe"
& $csc /nologo /optimize /platform:x64 /out:$recorder (Join-Path $PSScriptRoot "paste_recorder.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$inj (Join-Path $PSScriptRoot "paste_host_injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $recorder) -or -not (Test-Path $inj)) {
    Write-Host "FATAL: could not compile the recorder or the injector" -ForegroundColor Red
    exit 1
}

# See tests/test_issue684_paste_head.ps1: Set-Clipboard does not report a
# clipboard another window is holding open.
function Set-ClipboardVerified([string]$Value) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 5000) {
        try { Set-Clipboard -Value $Value -EA Stop } catch {}
        Start-Sleep -Milliseconds 100
        $now = try { (Get-Clipboard -Raw -EA Stop) } catch { $null }
        if ($null -ne $now -and $now.TrimEnd("`r", "`n") -eq $Value) { return $true }
    }
    return $false
}

# One gesture against a REAL attached client, with the recorder as the pane
# child so the assertion is on the bytes, not on the screen.
function Invoke-Gesture {
    param([string]$Tag, [string]$Mode, [int]$Delay, [string]$Text, [string]$Clip)
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $log = Join-Path $root "rec_$Tag.log"
    $sess = "i1049_$Tag"
    $res = [ordered]@{ Text = ""; Has200 = "?"; Rc = -1; Attached = $false; ClipSet = $false }

    & $PSMUX -L $NS new -d -s $sess -x 100 -y 30 -- $recorder $log 22 vt 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach-session", "-t", $sess -PassThru -WindowStyle Normal
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if (((& $PSMUX -L $NS display-message -t $sess -p '#{session_attached}') -join '').Trim() -eq '1') { $res.Attached = $true; break }
        Start-Sleep -Milliseconds 100
    }
    if ($res.Attached) {
        Start-Sleep -Seconds 2
        $res.ClipSet = Set-ClipboardVerified $Clip
        Start-Sleep -Milliseconds 250
        & $inj $client.Id $Mode $Delay $Text
        $res.Rc = $LASTEXITCODE
        Start-Sleep -Seconds 4
    }
    try { if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } } catch {}
    # The recorder writes its log when its 22 s window closes.
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^TEXT ' -Quiet)) -and $sw.ElapsedMilliseconds -lt 30000) { Start-Sleep -Milliseconds 500 }
    foreach ($l in Get-Content $log -EA SilentlyContinue) {
        if ($l -match '^TEXT (.*)$') { $res.Text = $Matches[1] }
        elseif ($l -match '^HAS200 (\w+)$') { $res.Has200 = $Matches[1] }
    }
    & $PSMUX -L $NS kill-session -t $sess 2>&1 | Out-Null
    [pscustomobject]$res
}

Write-Host "`n=== #1049: a held key whose repeats arrive in clumps ===" -ForegroundColor Yellow
# Five clumps of four, 133 ms apart: 30 repeats a second, bunched by the network.
$held = Invoke-Gesture -Tag "held" -Mode "clumps" -Delay 133 -Text "jjjj jjjj jjjj jjjj jjjj" -Clip "ZZZ not what is being typed"
if (-not $held.Attached) {
    Write-Skip "no client attached, nothing measurable"
} elseif ($held.Rc -eq 2) {
    Write-Skip "AttachConsole refused from this shell (run the suite from a real console)"
} else {
    if ($held.Text -eq ("j" * 20)) { Write-Pass "all 20 repeats arrived as keys, nothing else" }
    else { Write-Fail "the held key arrived as '$($held.Text)'" }
    if ($held.Has200 -eq "NO") { Write-Pass "a held key is never wrapped in bracketed paste" }
    else { Write-Fail "the held key was bracketed (200=$($held.Has200))" }
}

Write-Host "`n=== the guard: a pasted run of one character is still a paste ===" -ForegroundColor Yellow
$rule = "-" * 40
$run = Invoke-Gesture -Tag "run" -Mode "host" -Delay 150 -Text $rule -Clip $rule
if (-not $run.Attached -or $run.Rc -eq 2) {
    Write-Skip "paste arm not measurable on this host"
} elseif (-not $run.ClipSet) {
    Write-Skip "the clipboard never held the run, so the burst was not a paste of it"
} elseif ($run.Text -eq ("<ESC>[200~" + $rule + "<ESC>[201~")) {
    Write-Pass "the pasted run arrived whole and bracketed"
} else {
    Write-Fail "the pasted run changed shape: '$($run.Text)'"
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
$env:PSMUX_DATA_DIR = $savedDataDir
$env:PSMUX_NO_WARM = $savedNoWarm
if ($savedClip) { try { Set-Clipboard -Value $savedClip } catch {} }

Write-Host "`n================ SUMMARY ================" -ForegroundColor Yellow
Write-Host "  Passed: $script:Pass" -ForegroundColor Green
Write-Host "  Failed: $script:Fail" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
if ($script:Skip -gt 0) { Write-Host "  Skipped: $script:Skip" -ForegroundColor Yellow }
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
