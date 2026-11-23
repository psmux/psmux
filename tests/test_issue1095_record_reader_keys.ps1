# Issue #1095: on a console host that does not parse win32 input mode, a program that reads its keys as console records gets C-1, C-Space, C-i, C-j, C-m, C-BSpace and S-Enter as other keys.
#
# The inbox conhost of Windows 10 19045 never asks for win32 input mode
# (CSI ?9001h), so psmux writes tmux's legacy byte there (#845), and that host
# parses the byte into the wrong record. Measured with tests\record_key_child.cs
# in PSReadLine's mode (0x01E4) before the fix:
#
#   C-1, C-0   -> a plain 1, 0, no Ctrl     C-i -> Tab, no Ctrl
#   C-Space    -> Ctrl+Shift+2              C-j -> Ctrl+Enter
#   C-BSpace   -> Ctrl+H                    C-m -> Enter, no Ctrl
#   S-Enter    -> Alt+Enter
#
# psmux now writes those keys to a record reader as the record itself, with
# WriteConsoleInput, on such a host. On a host that parses win32 input mode
# (Windows 11) they already went in win32 form, so this suite expects the same
# records on both.
#
# Test 1: send-keys into the record reader in PSReadLine's mode (0x01E4) and
#         Far's (0x01B8): every key of the table arrives as its own record, and
#         C-a and a plain 1 keep theirs.
# Test 2: the same keys typed into an attached client (WriteConsoleInput into
#         the client's console): Ctrl+1, Ctrl+Backspace, Shift+Enter.  Not
#         Ctrl+J: on Windows 11 a typed Ctrl+J reaches the reader as
#         Ctrl+Enter, as the client's route writes tmux's byte 0a there.
# Test 3: a VT reader (0x03E4) keeps tmux's bytes for C-1 and C-BSpace.
#
# Isolation: own -L namespace per run, own PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server` and by the PID of the client this script started.
# PSMUX_TEST_BIN picks the binary (default: psmux on PATH).
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1095_record_reader_keys.ps1

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$root = Join-Path $env:TEMP "psmux_i1095_$(Get-Random)"
New-Item -ItemType Directory -Force $root | Out-Null
$saved = @{}
foreach ($v in 'PSMUX_DATA_DIR', 'PSMUX_NO_WARM', 'PSMUX_RECORD_LOG', 'PSMUX_RECORD_MODE', 'PSMUX_RUN_NOPAUSE') { $saved[$v] = [Environment]::GetEnvironmentVariable($v, 'Process') }
$env:PSMUX_DATA_DIR = $root
$env:PSMUX_NO_WARM = '1'
$env:PSMUX_RUN_NOPAUSE = '1'
$conf = Join-Path $root 'empty.conf'
'' | Set-Content -Path $conf -Encoding ASCII

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$injector = Join-Path $root 'injector.exe'
$recChild = Join-Path $root 'reckey.exe'
& $csc /nologo /platform:x64 /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$recChild "$PSScriptRoot\record_key_child.cs" 2>&1 | Out-Null
if (-not (Test-Path $injector) -or -not (Test-Path $recChild)) {
    Write-Host "  [FAIL] could not build the C# harnesses with $csc" -ForegroundColor Red
    exit 1
}

$script:Opened = @()
$script:Namespaces = @()

function Start-Client($ns, [string[]]$cmd) {
    $argv = @('-L', $ns, '-f', $conf, 'new-session', '-s', 'p', '-x', '100', '-y', '30', '--') + $cmd
    $p = Start-Process -FilePath $PSMUX -PassThru -ArgumentList $argv
    $script:Opened += $p
    $script:Namespaces += $ns
    $p
}
function Stop-Client($ns, $p) {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue }
}

# Starts the record logger in $mode under an attached client, then returns,
# for each action, the KEY down records it produced with the modifier keys'
# own records (vk 0x10, 0x11, 0x12) left out.  An action is a send-keys key,
# or INJ:<injector token> typed into the client's console.
function Probe-Records($mode, $actions) {
    $ns = "t1095$(Get-Random)"
    $log = Join-Path $root "rec_$mode.txt"
    Remove-Item $log -EA SilentlyContinue
    $env:PSMUX_RECORD_LOG = $log
    $env:PSMUX_RECORD_MODE = $mode
    $client = Start-Client $ns @($recChild)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 10000 -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 1500
    $out = [ordered]@{ mode = ((Get-Content $log -EA SilentlyContinue | Where-Object { $_ -match '^mode ' }) -join '') }
    $seen = @(Get-Content $log -EA SilentlyContinue).Count
    foreach ($a in $actions) {
        if ($a -like 'INJ:*') { & $injector $client.Id $a.Substring(4) 2>&1 | Out-Null }
        else { & $PSMUX -L $ns send-keys -t p $a 2>&1 | Out-Null }
        Start-Sleep -Milliseconds 700
        $all = @(Get-Content $log -EA SilentlyContinue)
        $out[$a] = @($all | Select-Object -Skip $seen | Where-Object { $_ -match '^KEY down' -and $_ -notmatch 'vk=0x1[012] ' })
        $seen = $all.Count
    }
    Stop-Client $ns $client
    $out
}

# One KEY down record with this virtual key and control key state (and this
# character, when $ch is given).
function Test-Record($label, $recs, [string]$vk, [string]$ctrl, [string]$ch) {
    $want = "vk=0x$vk" + $(if ($ch) { " ch=0x$ch" } else { '' }) + " ctrl=0x$ctrl"
    $hit = @($recs | Where-Object { $_ -match "vk=0x$vk " -and $_ -match "ctrl=0x$ctrl" -and (-not $ch -or $_ -match "ch=0x$ch ") })
    if ($recs.Count -eq 1 -and $hit.Count -eq 1) { Write-Pass "${label}: $($recs[0])" }
    else { Write-Fail "${label}: want one record $want, got $(if ($recs.Count) { $recs -join ' | ' } else { 'none' })" }
}

# key, vk, control key state, character (hex)
$table = @(
    @('C-1', '31', '0008', '0000'),
    @('C-0', '30', '0008', '0000'),
    @('C-Space', '20', '0008', '0000'),
    @('C-i', '49', '0008', '0009'),
    @('C-j', '4A', '0008', '000A'),
    @('C-m', '4D', '0008', '000D'),
    @('C-BSpace', '08', '0008', ''),
    @('S-Enter', '0D', '0010', '000D'),
    @('C-a', '41', '0008', '0001'),
    @('1', '31', '0000', '0031')
)

try {
    Write-Host "`n=== Issue #1095: keys a record reader would get as other keys ===" -ForegroundColor Cyan
    Write-Host "  Windows build $([Environment]::OSVersion.Version.Build)"

    Write-Host "`n[Test 1] send-keys into a record reader" -ForegroundColor Yellow
    foreach ($mode in '01E4', '01B8') {
        $r = Probe-Records $mode @($table | ForEach-Object { $_[0] })
        if ($r.mode -notmatch "-> 0x$mode") { Write-Skip "the child could not take mode 0x$mode ($($r.mode))"; continue }
        foreach ($t in $table) { Test-Record "0x$mode send-keys $($t[0])" $r[$t[0]] $t[1] $t[2] $t[3] }
    }

    Write-Host "`n[Test 2] the same keys typed into an attached client" -ForegroundColor Yellow
    $typed = @(
        @('Ctrl+1', 'INJ:{RAW:31:0000:0008}', '31', '0008', '0000'),
        @('Ctrl+Backspace', 'INJ:{RAW:08:007F:0008}', '08', '0008', ''),
        @('Shift+Enter', 'INJ:{MOD:0D:000D:0010}', '0D', '0010', '000D')
    )
    $r = Probe-Records '01E4' @($typed | ForEach-Object { $_[1] })
    if ($r.mode -notmatch '-> 0x01E4') { Write-Skip "the child could not take mode 0x01E4 ($($r.mode))" }
    else { foreach ($t in $typed) { Test-Record "typed $($t[0])" $r[$t[1]] $t[2] $t[3] $t[4] } }

    Write-Host "`n[Test 3] a VT reader keeps tmux's bytes" -ForegroundColor Yellow
    $r = Probe-Records '03E4' @('C-1', 'C-BSpace')
    if ($r.mode -notmatch '-> 0x03E4') { Write-Skip "the child could not take mode 0x03E4 ($($r.mode))" }
    else {
        foreach ($t in @(@('C-1', '0031'), @('C-BSpace', '0008'))) {
            $recs = $r[$t[0]]
            if ($recs.Count -eq 1 -and $recs[0] -match "ch=0x$($t[1]) ") { Write-Pass "VT reader $($t[0]): $($recs[0])" }
            else { Write-Fail "VT reader $($t[0]): want the byte 0x$($t[1]), got $(if ($recs.Count) { $recs -join ' | ' } else { 'none' })" }
        }
    }
}
finally {
    foreach ($ns in $script:Namespaces) { & $PSMUX -L $ns kill-server 2>&1 | Out-Null }
    foreach ($p in $script:Opened) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } }
    foreach ($v in $saved.Keys) { if ($null -eq $saved[$v]) { Remove-Item "Env:\$v" -EA SilentlyContinue } else { Set-Item "Env:\$v" $saved[$v] } }
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
