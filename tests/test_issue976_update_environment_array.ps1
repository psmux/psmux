# Issue #976: update-environment is tmux's array option (options-table.c:
# 1210-1220 at tag 3.8: a session option, OPTIONS_TABLE_IS_ARRAY, a value split
# on space and `,`, 12 default patterns, tmux 3.8 less MSYSTEM). psmux kept a list split on spaces:
# show printed one joined line, `-ga` changed nothing, `-a` without -g glued
# the text onto the last pattern (`XAUTHORITYFOO`), `update-environment[N]`
# was kept as text and never used, `-u update-environment[N]` removed nothing,
# and `A B,C` was two patterns.
#
# Every expected value is what tmux 3.8 (`-f /dev/null`, WSL) printed for the
# same commands. Without -g the session's own array starts empty, as tmux's
# session table does: an element or `-a` alone is the whole session array,
# `-u` drops it, and `-g` writes reach the global array meanwhile.
#
# [4] checks the patterns are used: a variable of the attaching client that
# matches an element set by index, or one added with -ga, reaches the session
# environment (environ.c:186-213, environ_update).
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server` plus the client PIDs this suite started.
# PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
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
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_uea_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
$script:NS = ""
$script:Clients = @()
function P { (& $PSMUX -L $script:NS @args 2>&1 | ForEach-Object { "$_" }) -join "`n" }

# A fresh server with $config as its config file.
function Fresh([string[]]$config = @()) {
    if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Value $config
    $script:NS = "uea_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    & $PSMUX -L $script:NS new-session -d -s s -x 80 -y 12 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        & $PSMUX -L $script:NS has-session -t s 2>$null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 500
}

$DEFAULT = @('DISPLAY', 'KRB5CCNAME', 'SSH_ASKPASS', 'SSH_AUTH_SOCK', 'SSH_AGENT_PID',
    'SSH_CONNECTION', 'WAYLAND_DISPLAY', 'WINDOWID', 'XAUTHORITY', 'XDG_CURRENT_DESKTOP',
    'XDG_SESSION_DESKTOP', 'XDG_SESSION_TYPE')
function Lines([string[]]$values, [int]$from = 0, [string]$mark = '') {
    $i = $from
    ($values | ForEach-Object { "update-environment[$i]$mark $_"; $i++ }) -join "`n"
}

Write-Host "`n=== update-environment is an array option ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] The default, as tmux 3.8 shows it" -ForegroundColor Yellow
    Fresh
    Check "show -g: one line per element, 12 patterns" (P show -g update-environment) (Lines $DEFAULT)
    Check "show -gv: the patterns alone" (P show -gv update-environment) ($DEFAULT -join "`n")
    Check "#{update-environment} joins them with a space" (P display -p '#{update-environment}') ($DEFAULT -join ' ')

    Write-Host "`n[2] -ga, [N], -gu [N], a split value, -gu" -ForegroundColor Yellow
    P set -ga update-environment FOO | Out-Null
    Check "-ga adds at the lowest free index" ((P show -g update-environment) -split "`n" | Select-Object -Last 1) "update-environment[12] FOO"
    P set -g 'update-environment[20]' BAR | Out-Null
    Check "[20] sets one element" (P show -g 'update-environment[20]') "update-environment[20] BAR"
    Check "#{update-environment[20]}" (P display -p '#{update-environment[20]}') "BAR"
    P set -gu 'update-environment[0]' | Out-Null
    Check "-gu [0] removes one, the others keep their index" (((P show -g update-environment) -split "`n" | Select-Object -First 2) -join "`n") "update-environment[1] KRB5CCNAME`nupdate-environment[2] SSH_ASKPASS"
    P set -g update-environment 'A B,C' | Out-Null
    Check "a value splits on space and ," (P show -g update-environment) (Lines @('A', 'B', 'C'))
    P set -g update-environment 'X' | Out-Null
    Check "a value without an index replaces the whole array" (P show -g update-environment) "update-environment[0] X"
    P set -gu update-environment | Out-Null
    Check "-gu puts the default back" (P show -g update-environment) (Lines $DEFAULT)

    Write-Host "`n[3] Without -g: the session's own array" -ForegroundColor Yellow
    Fresh
    P set 'update-environment[20]' X | Out-Null
    Check "an element alone is the whole session array" (P show update-environment) "update-environment[20] X"
    Check "the global array keeps its 12" (P show -g update-environment) (Lines $DEFAULT)
    Fresh
    P set -a update-environment FOO | Out-Null
    Check "-a with nothing before starts empty" (P show update-environment) "update-environment[0] FOO"
    Fresh
    P set update-environment 'A B' | Out-Null
    P set -a update-environment C | Out-Null
    Check "A B then -a C" (P show update-environment) (Lines @('A', 'B', 'C'))
    P set -u 'update-environment[0]' | Out-Null
    Check "then -u [0]" (P show update-environment) "update-environment[1] B`nupdate-environment[2] C"
    Fresh
    P set update-environment 'A B' | Out-Null
    P set -g update-environment G | Out-Null
    Check "-g while the session overrides writes the global array" (P show -g update-environment) "update-environment[0] G"
    Check "and the session keeps its own" (P show update-environment) (Lines @('A', 'B'))
    Fresh @("set -g 'update-environment[30]' Z")
    P set update-environment S | Out-Null
    P set -u update-environment | Out-Null
    Check "-u drops the session array and the global one applies again" ((P show -g update-environment) -split "`n" | Select-Object -Last 2) "update-environment[11] XDG_SESSION_TYPE update-environment[30] Z"
    Check "show -A marks what the session inherits" (((P show -A update-environment) -split "`n" | Select-Object -First 2) -join "`n") (Lines @('DISPLAY', 'KRB5CCNAME') 0 '*')

    Write-Host "`n[4] The elements are used on attach" -ForegroundColor Yellow
    Fresh @("set -g 'update-environment[20]' 'UEA_IDX*'")
    P set -ga update-environment UEA_APPENDED | Out-Null
    $env:UEA_IDX_ONE = 'by-index'
    $env:UEA_APPENDED = 'by-append'
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $script:NS, "attach", "-t", "s" -PassThru
    $script:Clients += $client.Id
    Remove-Item Env:UEA_IDX_ONE, Env:UEA_APPENDED
    $got = ''
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        $got = P show-environment -t s UEA_IDX_ONE
        if ($got -eq 'UEA_IDX_ONE=by-index') { break }
    }
    Check "a client variable matching [20] reaches the session" $got "UEA_IDX_ONE=by-index"
    Check "a client variable matching a -ga element reaches the session" (P show-environment -t s UEA_APPENDED) "UEA_APPENDED=by-append"
    Stop-Process -Id $client.Id -Force -EA SilentlyContinue

    Write-Host "`n[5] The config file gives the same array" -ForegroundColor Yellow
    Fresh @("set -g update-environment 'A B,C'", "set -ga update-environment D", "set -g 'update-environment[9]' E")
    Check "config: split, -ga and [9]" (P show -g update-environment) "$(Lines @('A', 'B', 'C', 'D'))`nupdate-environment[9] E"
}
catch {
    Write-Fail "exception: $_"
}
finally {
    foreach ($id in $script:Clients) { Stop-Process -Id $id -Force -EA SilentlyContinue }
    if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
