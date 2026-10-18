# Issue #849: a `;` in a config line ends a command.
#
# tmux reads a config file with its string grammar: "Each command is
# terminated by a newline or a semicolon (;)" (tmux.1:528), and an unquoted
# `;` ends the token it is in and the command with it (cmd-parse.y:1314 and
# :1675). `\;` stays in the line as an argument, which is how a key binding
# holds a list of commands, and a `;` inside quotes, a `{ }` block or a
# comment is not a separator.
#
# psmux handed the whole line to the parser, so `set -g @a1 on ; set -g @a2 on`
# set @a1 to `on ; set -g @a2 on`, an if-shell branch ran as one command, and
# `bind x a ; b` bound both commands where tmux binds `a` and runs `b`.
#
# Every expected value below is what tmux 3.7c printed for the same file on a
# fresh server (`tmux -L x -f /dev/null`, then `source-file`).
#
# The suite runs under -L in a PSMUX_DATA_DIR of its own and kills only its own
# namespace. Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_semi_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "semi" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "sc"

Write-Host "`n=== Issue #849: a semicolon in a config line ends a command ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
# An option never set reads "invalid option: NAME", so anything but the value
# fails the check.
function Opt($name) { ((P show-options -gv $name) -join '').Trim() }
function Opts([string[]]$names) { ($names | ForEach-Object { "$_=$(Opt $_)" }) -join ' ' }
function Unset([string[]]$names) { -not ($names | Where-Object { (Opt $_) -notmatch 'invalid option' }) }
function Key($table, $key) {
    ((P list-keys) | Where-Object { $_ -match "-T $table +$([regex]::Escape($key)) " } | ForEach-Object { ($_ -replace '\s+', ' ').Trim() }) -join '|'
}

# Write a one-file config of these lines and run the CLI `source-file` on it.
function Source([string[]]$lines) {
    $path = Join-Path $TMP "case.conf"
    $lines | Set-Content -Path $path -Encoding ASCII
    $err = Join-Path $TMP "err.txt"
    $p = Start-Process -FilePath $PSMUX -ArgumentList @("-L", $NS, "source-file", "`"$path`"") `
        -NoNewWindow -Wait -PassThru -RedirectStandardError $err -RedirectStandardOutput (Join-Path $TMP "out.txt")
    return @{ Code = $p.ExitCode; Err = ((Get-Content $err -Raw -EA SilentlyContinue) + "").Trim() }
}

function Reset-Session {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 500
}

try {
    # ── A. options ──
    Write-Head "A. set -g @a1 on ; set -g @a2 on"
    Reset-Session
    $r = Source @('set -g @a1 on ; set -g @a2 on')
    Check ($r.Code -eq 0) "exits 0 (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    $o = Opts @('@a1', '@a2')
    Check ($o -eq '@a1=on @a2=on') "both options are set (tmux)" "options read [$o]"

    Write-Head "B. a semicolon closing or inside a word"
    Reset-Session
    Source @('set -g @a1 on;set -g @a2 on;set -g @a3 a#b') | Out-Null
    $o = Opts @('@a1', '@a2', '@a3')
    Check ($o -eq '@a1=on @a2=on @a3=a#b') "three commands, and a # inside a word is not a comment (tmux)" "options read [$o]"

    Write-Head "C. the commands run in order"
    Reset-Session
    Source @('set -g @x a ; set -ga @x b ; set -ga @x c') | Out-Null
    Check ((Opt '@x') -eq 'abc') "@x is abc (tmux)" "@x reads [$(Opt '@x')]"

    Write-Head "D. a quoted semicolon is part of the value"
    Reset-Session
    Source @("set -g @a1 `"on;x`" ; set -g @a2 'c ; d'") | Out-Null
    Check ((Opt '@a1') -eq 'on;x') "double quotes keep on;x (tmux)" "@a1 reads [$(Opt '@a1')]"
    Check ((Opt '@a2') -eq 'c ; d') "single quotes keep c ; d (tmux)" "@a2 reads [$(Opt '@a2')]"

    Write-Head "E. a semicolon in a comment"
    Reset-Session
    Source @('set -g @a1 on # note ; set -g @a2 on') | Out-Null
    Check (((Opt '@a1') -eq 'on') -and (Unset @('@a2'))) "only @a1 is set (tmux)" "options read [$(Opts @('@a1', '@a2'))]"

    # ── key bindings ──
    Write-Head "F. bind x a ; b binds a and runs b"
    Reset-Session
    Source @('bind x set -g @a1 on ; set -g @a2 on') | Out-Null
    $k = Key 'prefix' 'x'
    Check ($k -match 'set -g @a1 on$') "x is bound to set -g @a1 on only (tmux)" "binding [$k]"
    Check (((Opt '@a2') -eq 'on') -and (Unset @('@a1'))) "set -g @a2 on ran at once (tmux)" "options read [$(Opts @('@a1', '@a2'))]"

    Write-Head "G. an escaped semicolon and a block still hold a list"
    Reset-Session
    Source @('bind x set -g @a1 on \; set -g @a2 on', 'bind y { set -g @a1 on ; set -g @a2 on }') | Out-Null
    $kx = Key 'prefix' 'x'; $ky = Key 'prefix' 'y'
    Check ($kx -match 'set -g @a1 on \\; set -g @a2 on$') "x holds both commands (tmux)" "binding [$kx]"
    Check ($ky -match 'set -g @a1 on \\; set -g @a2 on$') "y holds both commands (tmux)" "binding [$ky]"
    Check (Unset @('@a1', '@a2')) "nothing ran (tmux)" "options read [$(Opts @('@a1', '@a2'))]"

    # ── if-shell ──
    Write-Head "H. an if-shell branch with two commands"
    Reset-Session
    Source @('if-shell true "set -g @a1 on ; set -g @a2 on"') | Out-Null
    $o = Opts @('@a1', '@a2')
    Check ($o -eq '@a1=on @a2=on') "a quoted branch runs both (tmux)" "options read [$o]"
    Reset-Session
    Source @('if-shell true { set -g @a1 on ; set -g @a2 on }') | Out-Null
    $o = Opts @('@a1', '@a2')
    Check ($o -eq '@a1=on @a2=on') "a block runs both (tmux)" "options read [$o]"

    # ── commands ──
    Write-Head "I. options around a command on one line"
    Reset-Session
    Source @('set -g @a1 on ; new-window -n semi ; set -g @a2 on') | Out-Null
    Start-Sleep -Milliseconds 500
    $o = Opts @('@a1', '@a2')
    $wins = (P list-windows -F '#{window_name}') -join ','
    Check ($o -eq '@a1=on @a2=on') "both options are set (tmux)" "options read [$o]"
    Check ($wins -match '(^|,)semi(,|$)') "the window was made (tmux)" "windows [$wins]"

    # ── the startup config ──
    Write-Head "J. the startup config"
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    $saved = $env:PSMUX_CONFIG_FILE
    $startup = Join-Path $TMP "startup.conf"
    @("set -g default-shell $POWERSHELL", 'set -g @a1 on ; set -g @a2 on') | Set-Content -Path $startup -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $startup
    Reset-Session
    $env:PSMUX_CONFIG_FILE = $saved
    $o = Opts @('@a1', '@a2')
    Check ($o -eq '@a1=on @a2=on') "both options are set at startup (tmux)" "options read [$o]"

    # ── a line that starts with a semicolon ──
    Write-Head "K. a line that starts with ;"
    Reset-Session
    $r = Source @('; set -g @a2 on')
    Check ($r.Code -eq 1) "exits 1 (tmux: syntax error)" "exit $($r.Code), stderr [$($r.Err)]"
    Check (Unset @('@a2')) "nothing ran (tmux)" "@a2 reads [$(Opt '@a2')]"
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
