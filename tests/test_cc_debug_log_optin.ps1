# The control client diagnostics log (cc_debug.log) is opt in.
#
# Without PSMUX_CC_DEBUG a control client (psmux -C) writes no cc_debug.log
# and removes one an older build left in the data dir. With PSMUX_CC_DEBUG=1
# the log is written, but it holds event kinds, byte counts, timings and
# errors only: not the commands read from stdin, not the lines the server
# sent, not environment values. Control mode itself keeps working either way.
#
# Runs in a private PSMUX_DATA_DIR and a private -L namespace.
# Exe: $env:PSMUX_TEST_EXE, else the psmux on PATH.

$ErrorActionPreference = 'Continue'
$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
$ns = "cclogtest_$PID"
$dataDir = Join-Path ([IO.Path]::GetTempPath()) "psmux_cclog_$PID"
New-Item -ItemType Directory -Force $dataDir | Out-Null
$oldData = $env:PSMUX_DATA_DIR
$oldDbg = $env:PSMUX_CC_DEBUG
$env:PSMUX_DATA_DIR = $dataDir
$log = Join-Path $dataDir 'cc_debug.log'

$pass = 0; $fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { $script:pass++; Write-Host "  [PASS] $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  [FAIL] $name $detail" -ForegroundColor Red }
}

$keysMarker = 'ccmark-keys-7f3a91'
$pasteMarker = 'ccmark-paste-5b20c4'
$outMarker = 'ccmark-out-e81d06'

function Run-Control {
    $cmds = @(
        "send-keys -t demo -l '$keysMarker'",
        "set-buffer '$pasteMarker'",
        "display-message -p '$outMarker'"
    ) -join "`n"
    return (($cmds + "`n") | & $exe -L $ns -C new-session -A -s demo 2>&1 | Out-String)
}

Write-Host "=== cc_debug.log opt in ($exe) ==="

try {
    # 1. Default: no log, and a stale one is removed.
    Remove-Item Env:PSMUX_CC_DEBUG -ErrorAction SilentlyContinue
    Set-Content -Path $log -Value "stale log from an older build $keysMarker"
    $out = Run-Control
    Check 'default run: control replies arrive' ($out -match '%begin' -and $out -match '%end' -and $out -match [regex]::Escape($outMarker)) $out
    Check 'default run: stale cc_debug.log removed, none written' (-not (Test-Path $log))

    # 2. Opt in: log written, without contents or env values.
    $env:PSMUX_CC_DEBUG = '1'
    $out = Run-Control
    Check 'opt in run: control replies arrive' ($out -match '%begin' -and $out -match [regex]::Escape($outMarker)) $out
    Check 'opt in run: cc_debug.log written' (Test-Path $log)
    $text = if (Test-Path $log) { Get-Content $log -Raw } else { '' }
    Check 'opt in log records stdin byte counts' ($text -match 'IN  \(\d+ bytes\)') $text
    Check 'opt in log records output line kinds' ($text -match 'OUT %begin \(\d+ bytes\)') $text
    Check 'opt in log has no send-keys text' (-not $text.Contains($keysMarker))
    $keysHex = (([Text.Encoding]::ASCII.GetBytes($keysMarker) | ForEach-Object { '{0:x2}' -f $_ }) -join ' ')
    Check 'opt in log has no send-keys text in hex' (-not $text.Contains($keysHex))
    Check 'opt in log has no set-buffer text' (-not $text.Contains($pasteMarker))
    Check 'opt in log has no pane or reply output' (-not $text.Contains($outMarker))
    Check 'opt in log has no USERPROFILE value' (-not $text.Contains($env:USERPROFILE))

    # 3. PSMUX_CC_DEBUG=0 counts as off.
    $env:PSMUX_CC_DEBUG = '0'
    $out = Run-Control
    Check 'PSMUX_CC_DEBUG=0: log removed, none written' (-not (Test-Path $log))
} finally {
    & $exe -L $ns kill-server 2>&1 | Out-Null
    if ($null -eq $oldDbg) { Remove-Item Env:PSMUX_CC_DEBUG -ErrorAction SilentlyContinue } else { $env:PSMUX_CC_DEBUG = $oldDbg }
    if ($null -eq $oldData) { Remove-Item Env:PSMUX_DATA_DIR -ErrorAction SilentlyContinue } else { $env:PSMUX_DATA_DIR = $oldData }
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
}

Write-Host "Passed: $pass  Failed: $fail"
if ($fail -gt 0) { exit 1 } else { exit 0 }
