# switch-client from outside a client, to a session made after the client
# attached (found while testing #1028).
#
# tmux finds the client a switch-client acts on before anything else
# (cmd-queue.c, CMD_CLIENT_CFLAG at tag 3.8): -c names it, and with no -c
# outside any client it is the most recently active one (cmd_find_best_client).
# psmux runs a server per session, and a command from outside a pane went to
# the most recent session, which for a session made after the attach has no
# client: the command returned 0 and nothing moved.
#
# Each case opens an attached client in its own console window, which this
# suite closes by PID. Private USERPROFILE, PSMUX_DATA_DIR and config, a -L
# namespace per case; cleanup is `-L <ns> kill-server`.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_swn_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$script:NS = ""
$script:Win = $null
function P { & $PSMUX -L $script:NS @args 2>&1 }
# Sessions in `$before`, a client attached to `a`, then sessions in `$after`.
function Up($before, $after) {
    $script:NS = "swn_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    foreach ($s in $before) { P new-session -d -s $s -x 120 -y 30 | Out-Null }
    Start-Sleep -Milliseconds 1500
    $script:Win = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $script:NS, 'attach', '-t', 'a') -PassThru
    for ($k = 0; $k -lt 50 -and "$(P list-clients -t a)" -notmatch ': a \['; $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 800
    foreach ($s in $after) { P new-session -d -s $s -x 120 -y 30 | Out-Null }
    Start-Sleep -Milliseconds 1500
}
function Down {
    if ($script:Win -and -not $script:Win.HasExited) { & taskkill /PID $script:Win.Id /T /F 2>&1 | Out-Null }
    $script:Win = $null
    if ($script:NS) { P kill-server | Out-Null }
    $script:NS = ""
    Start-Sleep -Milliseconds 500
}
# The session the client is on, from every session's list-clients.
function On {
    $on = @()
    foreach ($s in 'a', 'b', 'c') {
        if ("$(P list-clients -t $s)" -match ": $s \[") { $on += $s }
    }
    $on -join ','
}
function Client { ("$(P list-clients -t a)" -split ':')[0] }
function Wait-On($s) {
    for ($k = 0; $k -lt 25 -and (On) -ne $s; $k++) { Start-Sleep -Milliseconds 200 }
}

Write-Host "`n=== switch-client to a session made after the client attached ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

try {
    Write-Host "  -- the session made after the attach" -ForegroundColor DarkCyan
    Up @('a') @('b')
    Check "attached to a" (On) 'a'
    $out = P switch-client -t b; $rc = $LASTEXITCODE
    Wait-On 'b'
    Check "switch-client -t b, exit" $rc 0
    Check "switch-client -t b moves the client" (On) 'b'
    P switch-client -t a | Out-Null
    Wait-On 'a'
    Check "and back to a" (On) 'a'
    Down

    Write-Host "  -- with -c" -ForegroundColor DarkCyan
    Up @('a') @('b')
    $cl = Client
    P switch-client -c $cl -t b | Out-Null
    Wait-On 'b'
    Check "switch-client -c <client> -t b moves it" (On) 'b'
    $err = "$(P switch-client -c /dev/pts/999999 -t a)"; $rc = $LASTEXITCODE
    Check "an unknown -c, exit" $rc 1
    Check "an unknown -c, message" ($err -match "can't find client: /dev/pts/999999") $true
    Check "and nothing moved" (On) 'b'
    Down

    Write-Host "  -- the session made before the attach (worked already)" -ForegroundColor DarkCyan
    Up @('a', 'b') @()
    P switch-client -t b | Out-Null
    Wait-On 'b'
    Check "switch-client -t b moves the client" (On) 'b'
    Down

    Write-Host "  -- two sessions made after the attach" -ForegroundColor DarkCyan
    Up @('a') @('b', 'c')
    P switch-client -t c | Out-Null
    Wait-On 'c'
    Check "switch-client -t c moves the client" (On) 'c'
    Down

    Write-Host "  -- no client anywhere" -ForegroundColor DarkCyan
    $script:NS = "swn_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s a -x 120 -y 30 | Out-Null
    P new-session -d -s b -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1500
    P switch-client -t b | Out-Null; $rc = $LASTEXITCODE
    Check "routing stays as it was, exit" $rc 0
    Down
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Down
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
