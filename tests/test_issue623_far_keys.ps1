# Issue #623 (third report): Far Manager's first F10 inserted a backslash into
# the command line instead of opening the quit dialog, and Ctrl+1 in the drives
# menu still opened the Temporary panel.
#
# F10.  Measured on 26200 with Far 3.0.6364, 3 runs each: natively the first
# F10 asked "Do you want to quit Far?" 3/3; in a psmux pane the command line
# already ended in `\` before any key, the first F10 only closed the
# autocompletion list that `\` had opened (0/3), and the second F10 worked.  The
# bytes psmux wrote for both F10s were identical (1b 5b 32 31 7e).  The `\` was
# the tail of a colour reply: Far reads its palette with one write of CSI 0c,
# OSC 4;0;?;...;255;? ST, CSI 0c, stops at the second DA1 reply, and ConPTY
# answers both DA1s itself before psmux has seen the OSC.  psmux then injected
# ESC ]10;.. ESC \ ESC ]11;.. ESC \ ESC ]4;0;.. ESC \ as key records into a
# console back in 0x01B8, and Far took them as typing.  The same happened to
# yazi with the XTVERSION reply.  Replies now go only to a console that is
# reading VT input (ENABLE_VIRTUAL_TERMINAL_INPUT).
#
# Ctrl+1.  Far with its mouse support off runs in 0x01E8 (no ENABLE_MOUSE_INPUT)
# and the Ctrl+digit record path was gated on the mouse bit, so Far got a bare
# `1`, the Temporary panel's hotkey.  Natively the same Ctrl+1 hid the disk type.
#
# Test 1 runs everywhere: tests/far_palette_query_child.cs sends Far's exact
# request from Far's record mode, the console state psmux found Far in at every
# reply it injected (10 of 10 launches), and counts the keys that arrive.  Tests 2 and 3 need Far Manager and SKIP without it;
# set PSMUX_TEST_FAR to a Far.exe (3.0.6364 reproduces the F10 half, newer
# builds no longer query the palette at startup).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SOCK = "i623fk"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }

$emptyConf = "$env:TEMP\psmux_623fk_empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$injector = "$env:TEMP\psmux_623fk_injector.exe"
$queryChild = "$env:TEMP\psmux_623fk_query.exe"
& $csc /nologo /platform:x64 /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$queryChild "$PSScriptRoot\far_palette_query_child.cs" 2>&1 | Out-Null
foreach ($exe in @($injector, $queryChild)) {
    if (-not (Test-Path $exe)) {
        Write-Host "  [FAIL] could not build the C# harnesses (csc at $csc): missing $exe" -ForegroundColor Red
        exit 1
    }
}

$dataRoot = "$env:TEMP\psmux_623fk_root"
if (-not (Test-Path $dataRoot)) { New-Item -ItemType Directory -Path $dataRoot | Out-Null }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$env:PSMUX_DATA_DIR = $dataRoot
$env:PSMUX_NO_WARM = "1"

function Cleanup-Session($name) {
    & $PSMUX -L $SOCK kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
}
function Capture($name) { (& $PSMUX -L $SOCK capture-pane -p -t $name 2>&1 | Out-String) }
# The injector writes records into the attached client's console; exit 2/3
# means the desktop refused the attach, which is not a psmux result.
function Inject($clientPid, $keys) {
    & $injector $clientPid $keys 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

Write-Host "`n=== Issue #623 Tests: Far's first function key and Ctrl+1 ===" -ForegroundColor Cyan

# =============================================================================
# TEST 1: a late colour reply must not reach a record reader as keystrokes
# =============================================================================
Write-Host "`n[Test 1] Far's DA1 bracketed palette query leaves no stray keys (record mode 0x01B8)" -ForegroundColor Yellow

$sess = "i623fk_q"
$qlog = "$dataRoot\far_query.txt"
Remove-Item $qlog -Force -EA SilentlyContinue
Cleanup-Session $sess
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $sess -x 100 -y 30 "$queryChild $qlog 4000" 2>&1 | Out-Null
$deadline = (Get-Date).AddSeconds(15)
while ((Get-Date) -lt $deadline -and -not ((Get-Content $qlog -EA SilentlyContinue) -match 'FAR_QUERY END')) {
    Start-Sleep -Milliseconds 300
}
$q = @(Get-Content $qlog -EA SilentlyContinue)
if (-not ($q -match 'FAR_QUERY END')) {
    Write-Fail "the query child never finished: $($q -join ' | ')"
} else {
    $da = ($q | Where-Object { $_ -match '^DA1 ' } | Select-Object -First 1)
    if ($da -eq 'DA1 2') {
        Write-Pass "ConPTY answered both DA1s itself, so Far's read window closed before psmux saw the OSC"
    } else {
        Write-Fail "the query did not see its two DA1 replies: $da"
    }
    $stray = ($q | Where-Object { $_ -match '^STRAY_KEYS' } | Select-Object -First 1)
    if ($stray -match '^STRAY_KEYS 0 ') {
        Write-Pass "no reply was typed into the record reader ($stray)"
    } else {
        Write-Fail "#623 regression: a late reply arrived as keystrokes ($stray)"
    }
}
Cleanup-Session $sess

# =============================================================================
# Far Manager
# =============================================================================
$far = $env:PSMUX_TEST_FAR
if (-not $far) {
    $far = @(
        "$env:ProgramFiles\Far Manager\Far.exe",
        "${env:ProgramFiles(x86)}\Far Manager\Far.exe",
        "$env:LOCALAPPDATA\Programs\Far Manager\Far.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
$shell = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $shell) { $shell = (Get-Command powershell -EA SilentlyContinue).Source }

# =============================================================================
# TEST 2: the first F10 and the first F12 work, and the command line is empty
# =============================================================================
Write-Host "`n[Test 2] Far started from the pane shell: first F10 / F12" -ForegroundColor Yellow

if (-not $far) {
    Write-Skip "Far Manager is not installed ('winget install --id FarManager.FarManager', or set PSMUX_TEST_FAR)"
} else {
    foreach ($case in @(@('{F10}', 'Do you want to quit Far\?', 'F10 quit dialog'),
                        @('{F12}', 'Screens', 'F12 screens list'))) {
        $sess2 = "i623fk_far"
        Cleanup-Session $sess2
        # The reporter's flow: a shell pane, then Far launched from it.
        $client = Start-Process -FilePath $PSMUX -PassThru `
            -ArgumentList "-L",$SOCK,"-f",$emptyConf,"new-session","-s",$sess2,"-x","120","-y","32","--","`"$shell`"","-NoLogo","-NoProfile"
        Start-Sleep -Seconds 4
        & $PSMUX -L $SOCK send-keys -t $sess2 -l "& '$far'" 2>&1 | Out-Null
        & $PSMUX -L $SOCK send-keys -t $sess2 Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 7
        $screen = Capture $sess2
        if ($screen -notmatch '1Help') {
            Write-Fail "Far did not start in the pane (no function key bar)"
        } else {
            $cmdline = (($screen -split "`n") | Where-Object { $_ -match '>' } | Select-Object -Last 1)
            if ($cmdline -match '>\\\s*') {
                Write-Fail "#623: Far's command line holds a stray backslash before any key: [$($cmdline.TrimEnd())]"
            } else {
                Write-Pass "the command line is empty before any key"
            }
            if (-not (Inject $client.Id $case[0])) {
                Write-Skip "injection refused by the desktop (not a psmux result)"
            } else {
                Start-Sleep -Seconds 2
                if ((Capture $sess2) -match $case[1]) {
                    Write-Pass "ONE key opened the $($case[2])"
                } else {
                    Write-Fail "#623: the first key did not open the $($case[2])"
                }
            }
        }
        if ($client -and -not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
        Cleanup-Session $sess2
    }
}

# =============================================================================
# TEST 3: Ctrl+1 in the drives menu with Far's mouse support switched off
# =============================================================================
Write-Host "`n[Test 3] Ctrl+1 in the drives menu, Far mouse support off (0x01E8)" -ForegroundColor Yellow

if (-not $far) {
    Write-Skip "Far Manager is not installed"
} else {
    $sess3 = "i623fk_nm"
    Cleanup-Session $sess3
    $client3 = Start-Process -FilePath $PSMUX -PassThru `
        -ArgumentList "-L",$SOCK,"-f",$emptyConf,"new-session","-s",$sess3,"-x","120","-y","32","--","`"$far`"","-set:Interface.Mouse=false"
    Start-Sleep -Seconds 8
    if ((Capture $sess3) -notmatch '1Help') {
        Write-Fail "Far did not start in the pane"
    } elseif (-not (Inject $client3.Id "{MOD:70:0000:0002}")) {
        Write-Skip "injection refused by the desktop (not a psmux result)"
    } else {
        Start-Sleep -Seconds 2
        $menu = Capture $sess3
        if ($menu -notmatch 'Change drive' -or $menu -notmatch 'fixed') {
            Write-Skip "the drives menu did not open with the disk type shown, nothing to toggle"
        } else {
            Inject $client3.Id "{RAW:31:0000:0008}" | Out-Null
            Start-Sleep -Seconds 2
            $after = Capture $sess3
            if ($after -match 'Temporary panel \[') {
                Write-Fail "#623: Ctrl+1 opened the Temporary panel, so Far saw a bare '1'"
            } elseif ($after -notmatch 'Change drive') {
                Write-Fail "#623: the drives menu closed on Ctrl+1"
            } elseif ($after -match 'fixed') {
                Write-Fail "#623: Ctrl+1 did not hide the disk type"
            } else {
                Write-Pass "ONE Ctrl+1 hid the disk type and the menu stayed open, as natively"
            }
        }
    }
    if ($client3 -and -not $client3.HasExited) { Stop-Process -Id $client3.Id -Force -EA SilentlyContinue }
    Cleanup-Session $sess3
}

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
$env:PSMUX_DATA_DIR = $savedDataDir
$env:PSMUX_NO_WARM = $savedNoWarm

Write-Host "`n=== Issue #623 Far keys summary ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
