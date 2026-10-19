# attach-session -t session:window[.pane] makes that window the session's
# current window and that pane its active pane before the client attaches,
# as tmux cmd-attach-session.c does (window_set_active_pane, then
# session_set_current). A window or pane that is not there is an error.
#
# Needs a visible client: each attach is started in its own console window
# and closed by its PID. Runs only under -L in a private PSMUX_DATA_DIR.

$ErrorActionPreference = 'Continue'
$Exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_test_attach_target_$PID"
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_RUN_NOPAUSE = '1'

$script:pass = 0
$script:fail = 0
function Check([string]$name, $got, $want) {
    if ("$got" -eq "$want") { $script:pass++; Write-Host "PASS $name ($got)" -ForegroundColor Green }
    else { $script:fail++; Write-Host "FAIL $name got '$got' want '$want'" -ForegroundColor Red }
}

# A session `aw` with window 0 split in two (pane 0 active) and window 1
# current, the layout every case starts from.
function New-Layout([string]$ns) {
    & $Exe -L $ns new -d -s aw | Out-Null
    & $Exe -L $ns splitw -t aw:0 | Out-Null
    & $Exe -L $ns select-pane -t aw:0.0 | Out-Null
    & $Exe -L $ns neww -t aw | Out-Null
}

function Wait-Client([string]$ns) {
    for ($i = 0; $i -lt 40; $i++) {
        $c = & $Exe -L $ns list-clients -F '#{client_name}' 2>$null | Select-Object -First 1
        if ($c) { return $c }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

function Test-Attach([string]$target, [string]$wantClient, [string]$label) {
    $ns = "test_attach_target_${PID}_$(Get-Random)"
    New-Layout $ns
    Check "$label before attach the session is on window 1" (& $Exe -L $ns display -t aw -p '#{window_index}.#{pane_index}') '1.0'
    $p = Start-Process -FilePath $Exe -ArgumentList @('-L', $ns, 'attach', '-t', $target) -PassThru
    try {
        $c = Wait-Client $ns
        if (-not $c) { $script:fail++; Write-Host "FAIL $label no client attached" -ForegroundColor Red; return }
        Start-Sleep -Milliseconds 500
        Check "$label client shows" (& $Exe -L $ns display -c $c -p '#{session_name}:#{window_index}.#{pane_index}') $wantClient
        Check "$label session current" (& $Exe -L $ns display -t aw -p 'aw:#{window_index}.#{pane_index}') $wantClient
    } finally {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        & $Exe -L $ns kill-server 2>$null | Out-Null
    }
}

Test-Attach 'aw:0'   'aw:0.0' 'attach -t aw:0'
Test-Attach 'aw:0.1' 'aw:0.1' 'attach -t aw:0.1'
Test-Attach 'aw'     'aw:1.0' 'attach -t aw (no window part)'
Test-Attach '=aw'    'aw:1.0' 'attach -t =aw (exact, no window part)'
Test-Attach 'aw:'    'aw:1.0' 'attach -t aw: (current window)'

# Window by id: ask the server which id window 0 has.
$ns = "test_attach_target_${PID}_$(Get-Random)"
New-Layout $ns
$wid = & $Exe -L $ns display -t aw:0 -p '#{window_id}'
& $Exe -L $ns kill-server 2>$null | Out-Null
Test-Attach "aw:$wid" 'aw:0.0' "attach -t aw:$wid (window id)"

# A window that is not there: tmux says `can't find window: 9` and exits 1,
# and nothing changes.
$ns = "test_attach_target_${PID}_$(Get-Random)"
New-Layout $ns
$err = & $Exe -L $ns attach -t aw:9 2>&1 | Out-String
Check 'attach -t aw:9 exit code' $LASTEXITCODE 1
Check 'attach -t aw:9 message' ($err.Trim() -replace '^psmux: ', '') "can't find window: 9"
$err = & $Exe -L $ns attach -t aw:0.7 2>&1 | Out-String
Check 'attach -t aw:0.7 exit code' $LASTEXITCODE 1
Check 'attach -t aw:0.7 message' ($err.Trim() -replace '^psmux: ', '') "can't find pane: 7"
Check 'failed attach leaves the session alone' (& $Exe -L $ns display -t aw -p '#{window_index}.#{pane_index}') '1.0'
& $Exe -L $ns kill-server 2>$null | Out-Null

Write-Host ""
Write-Host "Passed: $script:pass  Failed: $script:fail"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }
