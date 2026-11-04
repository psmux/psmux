# Issue #983: terminal-features is tmux's array option (options-table.c:
# 553-563 at tag 3.8: a server option, OPTIONS_TABLE_IS_ARRAY, a value split
# on `,`, three default entries). psmux kept it as text in its user options:
# show printed one line with an empty default, `-a` changed nothing, an
# element was kept as a separate text, and `-u terminal-features[N]` removed
# nothing.
#
# Every expected value is what tmux 3.8 (`-f /dev/null`, WSL) printed for the
# same commands. psmux keeps the array as data only; nothing reads it, so
# this suite checks set, show, #{} and the config file, not what a client
# draws.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_tfa_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
$script:NS = ""
function P { (& $PSMUX -L $script:NS @args 2>&1 | ForEach-Object { "$_" }) -join "`n" }
function Rc { & $PSMUX -L $script:NS @args 2>&1 | Out-Null; return $LASTEXITCODE }

# A fresh server with $config as its config file.
function Fresh([string[]]$config = @()) {
    if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Value $config
    $script:NS = "tfa_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    & $PSMUX -L $script:NS new-session -d -s s -x 80 -y 12 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        & $PSMUX -L $script:NS has-session -t s 2>$null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 500
}

$DEFAULT = @('xterm*:clipboard:ccolour:cstyle:focus:title', 'screen*:title', 'rxvt*:ignorefkeys')
function Lines([string[]]$values, [int]$from = 0) {
    $i = $from
    ($values | ForEach-Object { "terminal-features[$i] $_"; $i++ }) -join "`n"
}

Write-Host "`n=== terminal-features is an array option ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] tmux 3.8's default, one line per element" -ForegroundColor Yellow
    Fresh
    Check "show -s" (P show -s terminal-features) (Lines $DEFAULT)
    Check "show -sv" (P show -sv terminal-features) ($DEFAULT -join "`n")
    Check "show without -s" (P show terminal-features) (Lines $DEFAULT)
    Check "show -g" (P show -g terminal-features) (Lines $DEFAULT)
    Check "#{terminal-features}" (P display -p '#{terminal-features}') ($DEFAULT -join ' ')
    $listed = ((P show -s) -split "`n" | Where-Object { $_ -like 'terminal-features*' }) -join "`n"
    Check "the show -s listing has the elements" $listed (Lines $DEFAULT)

    Write-Host "`n[2] -a, an element, -u of one element" -ForegroundColor Yellow
    P set -as terminal-features ',xterm*:overline' | Out-Null
    Check "-as ',xterm*:overline' adds [3]" (P show -s terminal-features) (Lines ($DEFAULT + 'xterm*:overline'))
    P set -s 'terminal-features[7]' 'foo:bar' | Out-Null
    Check "show [7]" (P show -s 'terminal-features[7]') "terminal-features[7] foo:bar"
    Check "show [6], which is not set" (P show -s 'terminal-features[6]') "terminal-features[6] ''"
    P set -su 'terminal-features[0]' | Out-Null
    Check "-su [0] removes only [0]" (P show -s terminal-features) "$(Lines ($DEFAULT[1..2] + 'xterm*:overline') 1)`nterminal-features[7] foo:bar"

    Write-Host "`n[3] A whole value, -u of the array" -ForegroundColor Yellow
    P set -s terminal-features 'a,b c,,d' | Out-Null
    Check "'a,b c,,d' splits on , only" (P show -s terminal-features) "terminal-features[0] a`nterminal-features[1] `"b c`"`nterminal-features[2] d"
    P set -sa terminal-features e | Out-Null
    Check "-sa e takes the lowest free index" (P show -s terminal-features) "terminal-features[0] a`nterminal-features[1] `"b c`"`nterminal-features[2] d`nterminal-features[3] e"
    P set -su terminal-features | Out-Null
    Check "-su puts the default back" (P show -s terminal-features) (Lines $DEFAULT)
    P set terminal-features 'x:y' | Out-Null
    Check "set without -s" (P show -s terminal-features) "terminal-features[0] x:y"
    P set -g terminal-features 'g:h' | Out-Null
    Check "set -g" (P show -s terminal-features) "terminal-features[0] g:h"
    P set -s terminal-features '' | Out-Null
    Check "'' empties it" (P show -s terminal-features) "terminal-features"
    Check "-so on an empty array is refused" (P set -so terminal-features o) "already set: terminal-features"
    P set -su terminal-features | Out-Null

    Write-Host "`n[4] A named key, and a key that is no key" -ForegroundColor Yellow
    P set -s 'terminal-features[x]' named | Out-Null
    Check "[x] comes after the numbered elements" (P show -s terminal-features) "$(Lines $DEFAULT)`nterminal-features[x] named"
    Check "[] is refused" (Rc set -s 'terminal-features[]' z) "1"
    Check "and stores nothing" (P show -s terminal-features) "$(Lines $DEFAULT)`nterminal-features[x] named"

    Write-Host "`n[5] The config file gives the same array" -ForegroundColor Yellow
    Fresh @("set -as terminal-features ',xterm*:overline'", "set -s 'terminal-features[9]' 'foo:bar'", "set -su 'terminal-features[1]'")
    Check "config: -as, [9] and -su [1]" (P show -s terminal-features) "terminal-features[0] $($DEFAULT[0])`nterminal-features[2] $($DEFAULT[2])`nterminal-features[3] xterm*:overline`nterminal-features[9] foo:bar"
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
