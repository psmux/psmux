# refresh-client -l from a hook, a display-menu item and a confirm-before answer.
#
# These run on the in server route (commands.rs execute_command_string), which
# forwarded a bare "refresh-client" to the server port: the -l was dropped and
# no buffer was made.  A key binding and the command prompt were fine (the
# attached client sends those lines to the server whole).  Measured on
# 9b5c3e0e with a real attached client, keys by tests\injector.cs:
#
#   route                                   top buffer after   control (set -g @x)
#   bind -n F7 refresh-client -l            clipboard          -
#   C-b : refresh-client -l                 clipboard          -
#   set-hook after-new-window ...           OLDBUF             ran
#   confirm-before ... answered y           OLDBUF             ran
#   display-menu item                       OLDBUF             ran
#
# tmux runs -l on every route (cmd-refresh-client.c:237 tty_clipboard_query).
#
# Safety: private PSMUX_DATA_DIR, its own -L namespace, kill-server only with -L,
# the attached window and any clipboard holder are closed by PID, the user's
# clipboard is saved and restored, the holder caps itself at 5 s.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_refresh_client_l_server_route.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$root = Join-Path $env:TEMP "prsr_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "d"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$NS = "rclroute_$PID"
function P { & $PSMUX -L $NS @args 2>&1 }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$inj = Join-Path $root "injector.exe"
$holder = Join-Path $root "clipboard_holder.exe"
& $csc "-nologo" "-out:$inj" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc "-nologo" "-r:System.Windows.Forms.dll" "-out:$holder" (Join-Path $PSScriptRoot "clipboard_holder.cs") 2>&1 | Out-Null
if (-not (Test-Path $inj) -or -not (Test-Path $holder)) { Write-Host "FATAL: could not compile the injector or the clipboard holder" -ForegroundColor Red; exit 1 }

function Clip([string]$v) {
    for ($i = 0; $i -lt 50; $i++) {
        try { Set-Clipboard -Value $v -EA Stop } catch {}
        Start-Sleep -Milliseconds 50
        if ($(try { Get-Clipboard -Raw -EA Stop } catch { $null }) -eq $v) { return $true }
    }
    return $false
}
function Top { (P show-buffer) -join '' }
function Opt($n) { ((P show -gv $n) -join '').Trim() }
function Keys([string]$k) { & $inj $script:client.Id $k | Out-Null; return $LASTEXITCODE }
function Wait-Top([string]$want, [int]$ms = 2500) {
    $w = [Diagnostics.Stopwatch]::StartNew()
    while ($w.ElapsedMilliseconds -lt $ms) { if ((Top) -eq $want) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}

$savedClip = $null; $clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}
$script:client = $null
$script:holders = @()

Write-Host "`n=== refresh-client -l on the in server route ($PSMUX) ===" -ForegroundColor Cyan
try {
    if (-not (Clip "RSRPROBE")) { Write-Skip "the desktop refuses clipboard access to this process"; throw "noclip" }
    P new-session -d -s r -x 100 -y 30 | Out-Null
    Start-Sleep 3
    $script:client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach-session", "-t", "r" -PassThru -WindowStyle Normal
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000 -and ((P display -p '#{session_attached}') -join '').Trim() -ne '1') { Start-Sleep -Milliseconds 100 }
    if (((P display -p '#{session_attached}') -join '').Trim() -ne '1') { Write-Fail "the attached client never registered"; throw "noclient" }
    Start-Sleep 1

    Write-Host "`n[1] key binding and command prompt (client route, the control)" -ForegroundColor Yellow
    P bind -n F7 refresh-client -l | Out-Null
    P set-buffer OLDBUF | Out-Null; [void](Clip "RSRBIND")
    if ((Keys "{F7}") -eq 2) { Write-Skip "AttachConsole refused" }
    elseif (Wait-Top "RSRBIND") { Write-Pass "bind -n F7 refresh-client -l made the buffer" } else { Write-Fail "binding: top buffer [$(Top)]" }
    P set-buffer OLDBUF | Out-Null; [void](Clip "RSRPROMPT")
    [void](Keys "^b{SLEEP:300}:{SLEEP:300}refresh-client -l{ENTER}")
    if (Wait-Top "RSRPROMPT") { Write-Pass "C-b : refresh-client -l made the buffer" } else { Write-Fail "prompt: top buffer [$(Top)]" }

    Write-Host "`n[2] a hook" -ForegroundColor Yellow
    P set-buffer OLDBUF | Out-Null; [void](Clip "RSRHOOK")
    P set-hook -g after-new-window "refresh-client -l" | Out-Null
    P new-window | Out-Null
    $ok = Wait-Top "RSRHOOK"
    P set-hook -gu after-new-window | Out-Null
    P set-hook -g after-new-window "set -g @hk yes" | Out-Null; P new-window | Out-Null; Start-Sleep -Milliseconds 1000
    P set-hook -gu after-new-window | Out-Null
    $ran = (Opt '@hk') -eq 'yes'
    if (-not $ran) { Write-Fail "the after-new-window hook never ran (control)" }
    elseif ($ok) { Write-Pass "after-new-window refresh-client -l made the buffer" } else { Write-Fail "hook ran, top buffer [$(Top)], expected RSRHOOK" }

    Write-Host "`n[3] a confirm-before answer" -ForegroundColor Yellow
    P set-buffer OLDBUF | Out-Null; [void](Clip "RSRCONFIRM")
    P confirm-before -p "go?" "refresh-client -l" | Out-Null; Start-Sleep -Milliseconds 600
    [void](Keys "y")
    $ok = Wait-Top "RSRCONFIRM"
    P confirm-before -p "go?" "set -g @cf yes" | Out-Null; Start-Sleep -Milliseconds 600; [void](Keys "y"); Start-Sleep -Milliseconds 800
    if ((Opt '@cf') -ne 'yes') { Write-Fail "confirm-before never ran its command (control)" }
    elseif ($ok) { Write-Pass "confirm-before y ran refresh-client -l" } else { Write-Fail "confirm ran, top buffer [$(Top)], expected RSRCONFIRM" }

    Write-Host "`n[4] a display-menu item" -ForegroundColor Yellow
    P set-buffer OLDBUF | Out-Null; [void](Clip "RSRMENU")
    P display-menu -T m "Read clip" r "refresh-client -l" | Out-Null; Start-Sleep -Milliseconds 600
    [void](Keys "r")
    $ok = Wait-Top "RSRMENU"
    P display-menu -T m "Set" s "set -g @mn yes" | Out-Null; Start-Sleep -Milliseconds 600; [void](Keys "s"); Start-Sleep -Milliseconds 800
    if ((Opt '@mn') -ne 'yes') { Write-Fail "the menu item never ran (control)" }
    elseif ($ok) { Write-Pass "the menu item ran refresh-client -l" } else { Write-Fail "menu ran, top buffer [$(Top)], expected RSRMENU" }

    Write-Host "`n[5] a hook while another window holds the clipboard 400 ms" -ForegroundColor Yellow
    P set-buffer OLDBUF | Out-Null; [void](Clip "RSRHELD")
    $ready = Join-Path $root "ready_hook"
    $hp = Start-Process $holder -ArgumentList 400, $ready -PassThru -WindowStyle Hidden
    $script:holders += $hp
    $w = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path $ready) -and $w.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 2 }
    P set-hook -g after-new-window "refresh-client -l" | Out-Null
    $t = [Diagnostics.Stopwatch]::StartNew()
    P new-window | Out-Null
    $cliMs = $t.ElapsedMilliseconds
    $ok = Wait-Top "RSRHELD" 4000
    P set-hook -gu after-new-window | Out-Null
    $hp.WaitForExit(7000) | Out-Null
    if ($ok) { Write-Pass "the busy clipboard was waited for and stored (new-window returned in $cliMs ms)" } else { Write-Fail "held: top buffer [$(Top)], expected RSRHELD" }
    $alive = ((P display -p '#{session_attached}') -join '').Trim()
    if ($alive -eq '1') { Write-Pass "the server and the client stayed responsive" } else { Write-Fail "session_attached=$alive" }
}
catch { if ($_.Exception.Message -notin 'noclip', 'noclient') { Write-Fail "unexpected: $_" } }
finally {
    foreach ($h in $script:holders) { if ($h -and -not $h.HasExited) { Stop-Process -Id $h.Id -Force -EA SilentlyContinue } }
    if ($script:client) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
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
