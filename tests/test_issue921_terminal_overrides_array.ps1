# Issue #921: terminal-overrides as tmux's array option. Each element keeps
# its own index, so `set -s terminal-overrides[N]` and `set -su
# terminal-overrides[N]` work on one element and `show` prints the indexes as
# they are; a new server holds tmux's default `linux*:AX@`; a value is split
# on `,` with its spaces kept; and `#{terminal-overrides}` joins the elements
# with a space.
#
# Every expected line below is what tmux 3.7c (`-f /dev/null`, WSL) printed
# for the same sequence of commands.
#
# Isolated: USERPROFILE, PSMUX_DATA_DIR and PSMUX_CONFIG_FILE point into a
# fresh temp dir and every command uses -L. PSMUX_EXE picks the binary
# (defaults to psmux on PATH).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$TempHome = Join-Path $env:TEMP ("psmux_tov_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "empty.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_TARGET*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$NS = "tov_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function P { & $PSMUX -L $NS @args 2>&1 }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}
function Show { (P show -s terminal-overrides) -join ' | ' }

try {
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t s | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }

    Write-Host "`n[0] a new server holds tmux's default" -ForegroundColor Yellow
    Check "show -s terminal-overrides" (Show) 'terminal-overrides[0] linux*:AX@'

    Write-Host "`n[1] a value without an index replaces the array" -ForegroundColor Yellow
    P set -s terminal-overrides 'a:smcup@,b:rmcup@' | Out-Null
    Check "show" (Show) 'terminal-overrides[0] a:smcup@ | terminal-overrides[1] b:rmcup@'

    Write-Host "`n[2] -a adds elements, with or without a leading comma" -ForegroundColor Yellow
    P set -sa terminal-overrides ',c:Tc' | Out-Null
    P set -sa terminal-overrides 'd:Tc' | Out-Null
    Check "show" (Show) 'terminal-overrides[0] a:smcup@ | terminal-overrides[1] b:rmcup@ | terminal-overrides[2] c:Tc | terminal-overrides[3] d:Tc'

    Write-Host "`n[3] terminal-overrides[N] sets one element" -ForegroundColor Yellow
    P set -s 'terminal-overrides[10]' 'e:smcup@' | Out-Null
    Check "show" (Show) 'terminal-overrides[0] a:smcup@ | terminal-overrides[1] b:rmcup@ | terminal-overrides[2] c:Tc | terminal-overrides[3] d:Tc | terminal-overrides[10] e:smcup@'

    Write-Host "`n[4] -u terminal-overrides[N] removes one, the rest keep their indexes" -ForegroundColor Yellow
    P set -su 'terminal-overrides[0]' | Out-Null
    Check "show" (Show) 'terminal-overrides[1] b:rmcup@ | terminal-overrides[2] c:Tc | terminal-overrides[3] d:Tc | terminal-overrides[10] e:smcup@'

    Write-Host "`n[5] -a takes the lowest free index" -ForegroundColor Yellow
    P set -sa terminal-overrides 'f:Tc' | Out-Null
    Check "show" (Show) 'terminal-overrides[0] f:Tc | terminal-overrides[1] b:rmcup@ | terminal-overrides[2] c:Tc | terminal-overrides[3] d:Tc | terminal-overrides[10] e:smcup@'

    Write-Host "`n[6] reading it back" -ForegroundColor Yellow
    Check "show -sv" ((P show -sv terminal-overrides) -join ' | ') 'f:Tc | b:rmcup@ | c:Tc | d:Tc | e:smcup@'
    Check "show -s terminal-overrides[10]" ((P show -s 'terminal-overrides[10]') -join ' | ') 'terminal-overrides[10] e:smcup@'
    Check "show -sv terminal-overrides[10]" ((P show -sv 'terminal-overrides[10]') -join ' | ') 'e:smcup@'
    Check "#{terminal-overrides}" ((P display -p '#{terminal-overrides}') -join ' | ') 'f:Tc b:rmcup@ c:Tc d:Tc e:smcup@'
    Check "#{terminal-overrides[10]}" ((P display -p '#{terminal-overrides[10]}') -join ' | ') 'e:smcup@'

    Write-Host "`n[7] -a on an element appends to its text" -ForegroundColor Yellow
    P set -sa 'terminal-overrides[10]' ':rmcup@' | Out-Null
    Check "show -s terminal-overrides[10]" ((P show -s 'terminal-overrides[10]') -join ' | ') 'terminal-overrides[10] e:smcup@:rmcup@'

    Write-Host "`n[8] a bad index is refused and stores nothing" -ForegroundColor Yellow
    $out = (P set -s 'terminal-overrides[x]' 'z') -join ' | '
    $rc = $LASTEXITCODE
    # tmux 3.7c fails too (rc=1); its message is `ambiguous option`, psmux says
    # `invalid option` as it does for command-alias[x].
    if ($rc -ne 0 -and $out -match 'invalid option: terminal-overrides\[x\]') { Write-Pass "set terminal-overrides[x] fails: rc=$rc '$out'" } else { Write-Fail "set terminal-overrides[x] rc=$rc '$out'" }
    Check "show after the bad index" (Show) 'terminal-overrides[0] f:Tc | terminal-overrides[1] b:rmcup@ | terminal-overrides[2] c:Tc | terminal-overrides[3] d:Tc | terminal-overrides[10] e:smcup@:rmcup@'

    Write-Host "`n[9] -u puts tmux's default back" -ForegroundColor Yellow
    P set -su terminal-overrides | Out-Null
    Check "show" (Show) 'terminal-overrides[0] linux*:AX@'

    Write-Host "`n[10] spaces around an element are kept" -ForegroundColor Yellow
    P set -s terminal-overrides 'a:smcup@, b:rmcup@ ,c' | Out-Null
    Check "show" (Show) 'terminal-overrides[0] a:smcup@ | terminal-overrides[1] " b:rmcup@ " | terminal-overrides[2] c'
    Check "show -sv" (((P show -sv terminal-overrides) | ForEach-Object { "[$_]" }) -join ' | ') '[a:smcup@] | [ b:rmcup@ ] | [c]'

    Write-Host "`n[11] an empty value empties the array" -ForegroundColor Yellow
    P set -s terminal-overrides '' | Out-Null
    Check "show" (Show) 'terminal-overrides'

    Write-Host "`n[12] a config file uses tmux's spelling" -ForegroundColor Yellow
    $conf = Join-Path $TempHome "tov.conf"
    Set-Content -Path $conf -Value @("set -su terminal-overrides", "set -s terminal-overrides[5] '*:smcup@:rmcup@'", "set -sa terminal-overrides 'xterm*:Tc'")
    P source-file $conf | Out-Null
    Check "show" (Show) 'terminal-overrides[0] linux*:AX@ | terminal-overrides[1] xterm*:Tc | terminal-overrides[5] *:smcup@:rmcup@'
}
finally {
    P kill-server | Out-Null
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
