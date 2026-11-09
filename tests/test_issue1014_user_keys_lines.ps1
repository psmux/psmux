# Issue #1014: a user-keys value set from the command prompt, a key binding or
# a hook line that names another session kept `\e` as the two characters `\`
# and `e`; only the config file's `set` line read it as ESC. tmux reads every
# one of those lines with its parser, where `\e` bare or in double quotes is
# ESC and a single quoted backslash is literal (cmd-parse.y:1701-1705 and
# :1478 at tag 3.8). An argument from the shell's command line is taken as it
# is (cmd_parse_from_arguments does not lex), so the CLI keeps `\e`.
#
#   [1] the command prompt, typed into a client under a pseudoconsole on the
#       VT route, and then the sequence itself runs `bind -n User0`
#   [2] a binding (`bind y set -s user-keys[0] "\e[99~"`), and the sequence
#       runs `bind -n User0`; a single quoted value stays literal
#   [3] the prompt with `-t B:`, forwarded to B
#   [4] a hook on A whose line names B (`-t =B:`), forwarded to B
#   [5] the CLI keeps the backslash (tmux 3.8: `user-keys[6] \\e[96~`)
#   [6] keystrokes by WriteConsoleInput (tests\injector.cs) into a real
#       console client: C-b : and the line, and C-b y
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server` plus the PIDs this suite started.
# PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:OpenedPids = @()
$script:Namespaces = @()
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_uk1014_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$VTHOST = Join-Path $TempHome "conpty_vt_keys.exe"
$INJ = Join-Path $TempHome "injector.exe"
if ($csc) {
    & $csc /nologo /optimize "/out:$VTHOST" (Join-Path $PSScriptRoot "conpty_vt_keys.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$INJ" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}
if (-not ((Test-Path $VTHOST) -and (Test-Path $INJ))) { Write-Error "could not build tests\conpty_vt_keys.cs or tests\injector.cs"; exit 1 }

$POWERSHELL = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $POWERSHELL) { $POWERSHELL = (Get-Command powershell).Source }
$SHELLCONF = "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'"

function New-Ns { $n = "uk14_" + [guid]::NewGuid().ToString('N').Substring(0, 6); $script:Namespaces += $n; return $n }
function P($ns) { (& $PSMUX -L $ns @args 2>&1 | ForEach-Object { "$_" }) -join "`n" }
function Write-Conf($lines) {
    $path = Join-Path $TempHome ("c_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".conf")
    ((@($lines) + $SHELLCONF) -join "`n") + "`n" | Set-Content $path -Encoding ASCII
    return $path
}
# conpty_vt_keys steps for a string: its bytes, comma separated hex.
function Hex([string]$s) { ([Text.Encoding]::UTF8.GetBytes($s) | ForEach-Object { '{0:x2}' -f $_ }) -join ',' }
# The value as hex code points, the way the issue reports it.
function Value($ns, $t, $el = 'user-keys[0]') {
    $v = [string](& $PSMUX -L $ns show-options -sv -t $t $el 2>$null)
    return (($v.ToCharArray() | ForEach-Object { '{0:X2}' -f [int]$_ }) -join ' ')
}
function Wait-Session($ns, $s) {
    for ($i = 0; $i -lt 60; $i++) { & $PSMUX -L $ns has-session -t $s 2>$null; if ($LASTEXITCODE -eq 0) { return }; Start-Sleep -Milliseconds 250 }
}
# A client of session $s under the pseudoconsole host on the VT route (the
# one that matches user-keys sequences). It comes up in 7 s, then the host
# writes $steps and holds the console a little; the session lives on.
$clientVars = @("PSMUX_SESSION", "PSMUX_SESSION_NAME", "PSMUX_PANE", "PSMUX_TARGET_SESSION", "TMUX", "TMUX_PANE", "PSMUX_VT_INPUT")
function Run-VtClient($ns, $conf, $s, $steps) {
    foreach ($k in $clientVars) { Remove-Item "Env:\$k" -EA SilentlyContinue }
    $env:PSMUX_VT_INPUT = "1"
    $env:PSMUX_CONFIG_FILE = $conf
    $p = Start-Process -FilePath $VTHOST -WindowStyle Hidden -PassThru `
        -ArgumentList @("w7000,$steps,h1500", "`"$PSMUX`"", "-L", $ns, "new-session", "-A", "-s", $s)
    $script:OpenedPids += $p.Id
    Remove-Item Env:PSMUX_VT_INPUT -EA SilentlyContinue
    if (-not $p.WaitForExit(40000)) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue }
}

$E = [char]27
$ESCVAL = "1B 5B 39 39 7E"          # ESC [ 9 9 ~
$LITERAL = "5C 65 5B 39 39 7E"      # \ e [ 9 9 ~
$SEQ = Hex "$E[99~"
$CB = "02"
$PROMPTLINE = 'set -s user-keys[0] "\e[99~"'

Write-Host "`n=== #1014 user-keys from the prompt, a binding and a hook ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

try {
    Write-Host "`n[1] The command prompt, then the sequence" -ForegroundColor Yellow
    $ns = New-Ns
    $conf = Write-Conf @('bind -n User0 set -g @hit yes')
    Run-VtClient $ns $conf "A" "$CB,w400,3a,w500,$(Hex $PROMPTLINE),w300,0d,w1500,$SEQ,w1000"
    Check "the prompt's value is ESC[99~" (Value $ns A) $ESCVAL
    Check "the typed sequence runs bind -n User0" (P $ns show -gqv '@hit') "yes"
    P $ns kill-server | Out-Null

    Write-Host "`n[2] A binding, then the sequence" -ForegroundColor Yellow
    $ns = New-Ns
    $conf = Write-Conf @('bind y set -s user-keys[0] "\e[99~"', 'bind -n User0 set -g @hit yes')
    Run-VtClient $ns $conf "A" "$CB,w400,79,w1500,$SEQ,w1000"
    Check "the binding's value is ESC[99~" (Value $ns A) $ESCVAL
    Check "the typed sequence runs bind -n User0" (P $ns show -gqv '@hit') "yes"
    P $ns kill-server | Out-Null

    $ns = New-Ns
    $conf = Write-Conf @("bind y set -s user-keys[0] '\e[99~'")
    Run-VtClient $ns $conf "A" "$CB,w400,79,w1500"
    Check "a single quoted value in a binding keeps its backslash" (Value $ns A) $LITERAL
    P $ns kill-server | Out-Null

    Write-Host "`n[3] The prompt with -t B:" -ForegroundColor Yellow
    $ns = New-Ns
    $env:PSMUX_CONFIG_FILE = Write-Conf @()
    & $PSMUX -L $ns new-session -d -s B 2>&1 | Out-Null
    Wait-Session $ns B
    Run-VtClient $ns $env:PSMUX_CONFIG_FILE "A" "$CB,w400,3a,w500,$(Hex 'set -s -t B: user-keys[0] "\e[99~"'),w300,0d,w1500"
    Check "B's value is ESC[99~" (Value $ns B) $ESCVAL
    P $ns kill-server | Out-Null

    Write-Host "`n[4] A hook on A whose line names B" -ForegroundColor Yellow
    $ns = New-Ns
    $env:PSMUX_CONFIG_FILE = Write-Conf @("set-hook -g after-split-window 'set -s -t =B: user-keys[0] `"\e[99~`"'")
    & $PSMUX -L $ns new-session -d -s B 2>&1 | Out-Null
    Wait-Session $ns B
    & $PSMUX -L $ns new-session -d -s A 2>&1 | Out-Null
    Wait-Session $ns A
    Start-Sleep -Milliseconds 800
    P $ns split-window -t A | Out-Null
    Start-Sleep -Milliseconds 1500
    Check "B's value is ESC[99~" (Value $ns B) $ESCVAL
    P $ns kill-server | Out-Null

    Write-Host "`n[5] The shell's command line keeps the backslash" -ForegroundColor Yellow
    $ns = New-Ns
    $env:PSMUX_CONFIG_FILE = Write-Conf @()
    & $PSMUX -L $ns new-session -d -s A 2>&1 | Out-Null
    Wait-Session $ns A
    P $ns set -s 'user-keys[0]' '\e[99~' | Out-Null
    Check "set from the CLI keeps \e" (Value $ns A) $LITERAL
    P $ns set -s 'user-keys[0]' "$E[99~" | Out-Null
    Check "an ESC from the CLI is ESC" (Value $ns A) $ESCVAL
    P $ns kill-server | Out-Null

    Write-Host "`n[6] Keystrokes by WriteConsoleInput into a console client" -ForegroundColor Yellow
    $ns = New-Ns
    $env:PSMUX_CONFIG_FILE = Write-Conf @("bind y set -s user-keys[1] `"\eOz`"")
    & $PSMUX -L $ns new-session -d -s A 2>&1 | Out-Null
    Wait-Session $ns A
    Start-Sleep -Milliseconds 800
    $c = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $ns, 'attach', '-t', 'A') -PassThru
    $script:OpenedPids += $c.Id
    Start-Sleep -Seconds 4
    & $INJ $c.Id "^b{SLEEP:500}:{SLEEP:400}$PROMPTLINE{ENTER}" | Out-Null
    Start-Sleep -Milliseconds 1500
    Check "the prompt typed by WriteConsoleInput gives ESC[99~" (Value $ns A) $ESCVAL
    & $INJ $c.Id "^b{SLEEP:500}y" | Out-Null
    Start-Sleep -Milliseconds 1500
    Check "C-b y typed by WriteConsoleInput gives ESC O z" (Value $ns A 'user-keys[1]') "1B 4F 7A"
    P $ns kill-server | Out-Null
}
catch {
    Write-Fail "exception: $_"
}
finally {
    foreach ($n in $script:Namespaces) { & $PSMUX -L $n kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    foreach ($id in $script:OpenedPids) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
