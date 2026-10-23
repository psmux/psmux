# choose-tree -s and -w open the choosers prefix+s and prefix+w open, on
# every route.
#
# tmux 3.7c: choose-window and choose-session are default command aliases for
# `choose-tree -w` and `choose-tree -s` (options-table.c), prefix+s and
# prefix+w are `choose-tree -Zs` and `choose-tree -Zw` (key-bindings.c), and
# `-s` wins over `-w` (window-tree.c, window_tree_init). Before the fix no
# route read either flag:
#   - a prefix binding to `choose-tree -Zs` opened the window tree
#   - `:choose-tree -s` and a root table binding showed list-tree JSON in a
#     text popup
#   - `psmux choose-tree -s` and the in process route (hooks, menus,
#     confirm-before) opened nothing at all
#
# Live: a real attached client, keys through WriteConsoleInput
# (tests/injector.cs) and the client's console read back (tests/conread.cs).
# Isolated: -L, and USERPROFILE, PSMUX_DATA_DIR and PSMUX_CONFIG_FILE point
# into a temp dir. PSMUX_EXE picks the binary (defaults to target\release).

$ErrorActionPreference = "Continue"
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ctf_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
. "$PSScriptRoot\isolated_config.ps1" -Lines @(
    'bind S choose-tree -Zs',
    'bind W choose-tree -Zw',
    'bind X choose-tree -sZ',
    'bind Y choose-window -s',
    'bind -n F5 choose-tree -s',
    'bind -n F6 choose-tree -w'
)
$NS = "ctf_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "ctf"
function P { & $PSMUX -L $NS @args 2>&1 }

Write-Host "`n=== choose-tree -s / -w on every route ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((P -V) -join ' ')"

$toolDir = Join-Path $TempHome "tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $toolDir "injector.exe"
$READ = Join-Path $toolDir "conread.exe"
& $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
if (-not ((Test-Path $INJ) -and (Test-Path $READ))) { Write-Fail "harness tools did not compile"; exit 1 }

# The titles the two choosers draw. A JSON text popup is titled
# `choose-tree` too, so the marker includes the key help after it.
$TREE = "choose-tree (0-9=jump"
$SESSION = "choose-session (f=filter"

function Start-Client {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach", "-t", $S -PassThru
    Start-Sleep -Seconds 4
    return $p
}
function Stop-Client($p) {
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 700
}
function Send-Keys($p, $keys) {
    & $INJ $p.Id $keys | Out-Null
    Start-Sleep -Milliseconds 900
}
function Read-Screen($p) { return ([string[]](& $READ $p.Id)) -join "`n" }
function Chooser-On($screen) {
    if ($screen.Contains($SESSION)) { return "session" }
    if ($screen.Contains($TREE)) { return "tree" }
    return "none"
}

# Open a client, run $open (keys, or a script block run from outside), and
# check which chooser it drew.
function Check-Route($name, $open, $want) {
    $c = Start-Client
    if ($open -is [scriptblock]) { & $open $c } else { Send-Keys $c $open }
    Start-Sleep -Milliseconds 1200
    $got = Chooser-On (Read-Screen $c)
    if ($got -eq $want) { Write-Pass "$name opens the $want chooser" } else { Write-Fail "$name expected the $want chooser, got $got" }
    Stop-Client $c
}

try {
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P new-window -d -t $S | Out-Null
    P has-session -t $S | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }
    P bind-key V choose-tree -Zs | Out-Null

    Write-Host "`n--- the default bindings stay as they were ---" -ForegroundColor Cyan
    Check-Route "prefix w" "^b{SLEEP:500}w{SLEEP:1500}" "tree"
    Check-Route "prefix s" "^b{SLEEP:500}s{SLEEP:1500}" "session"

    Write-Host "`n--- prefix bindings ---" -ForegroundColor Cyan
    Check-Route "config 'bind S choose-tree -Zs'" "^b{SLEEP:500}S{SLEEP:1500}" "session"
    Check-Route "config 'bind W choose-tree -Zw'" "^b{SLEEP:500}W{SLEEP:1500}" "tree"
    Check-Route "config 'bind X choose-tree -sZ'" "^b{SLEEP:500}X{SLEEP:1500}" "session"
    Check-Route "config 'bind Y choose-window -s'" "^b{SLEEP:500}Y{SLEEP:1500}" "session"
    Check-Route "runtime 'bind-key V choose-tree -Zs'" "^b{SLEEP:500}V{SLEEP:1500}" "session"

    Write-Host "`n--- root table bindings ---" -ForegroundColor Cyan
    Check-Route "bind -n F5 choose-tree -s" "{F5}{SLEEP:1500}" "session"
    Check-Route "bind -n F6 choose-tree -w" "{F6}{SLEEP:1500}" "tree"

    Write-Host "`n--- the command prompt ---" -ForegroundColor Cyan
    Check-Route ":choose-tree -s" "^b{SLEEP:500}:{SLEEP:600}choose-tree -s{ENTER}{SLEEP:1500}" "session"
    Check-Route ":choose-tree -w" "^b{SLEEP:500}:{SLEEP:600}choose-tree -w{ENTER}{SLEEP:1500}" "tree"
    Check-Route ":choose-tree" "^b{SLEEP:500}:{SLEEP:600}choose-tree{ENTER}{SLEEP:1500}" "tree"
    Check-Route ":choose-session" "^b{SLEEP:500}:{SLEEP:600}choose-session{ENTER}{SLEEP:1500}" "session"
    Check-Route ":choose-window -s" "^b{SLEEP:500}:{SLEEP:600}choose-window -s{ENTER}{SLEEP:1500}" "session"

    Write-Host "`n--- the CLI ---" -ForegroundColor Cyan
    Check-Route "psmux choose-tree -s" { param($c) P choose-tree -s -t $S | Out-Null } "session"
    Check-Route "psmux choose-tree -Zw" { param($c) P choose-tree -Zw -t $S | Out-Null } "tree"
    $out = P choose-tree -s -t $S
    if ($LASTEXITCODE -eq 0 -and -not "$out") { Write-Pass "psmux choose-tree -s prints nothing, exit 0" } else { Write-Fail "psmux choose-tree -s printed '$out', exit $LASTEXITCODE" }

    Write-Host "`n--- the in process route (a hook) ---" -ForegroundColor Cyan
    Check-Route "after-new-window hook 'choose-tree -s'" { param($c) P set-hook -t $S after-new-window "choose-tree -s" | Out-Null; P new-window -d -t $S | Out-Null } "session"
    Check-Route "after-new-window hook 'choose-tree -w'" { param($c) P set-hook -t $S after-new-window "choose-tree -w" | Out-Null; P new-window -d -t $S | Out-Null } "tree"
    P set-hook -u -t $S after-new-window | Out-Null
}
finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed ===" -ForegroundColor $(if ($script:TestsFailed -eq 0) { 'Green' } else { 'Yellow' })
exit $script:TestsFailed
