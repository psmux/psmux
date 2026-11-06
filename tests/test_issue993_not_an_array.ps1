# Issue #993: set-option on an element of an option that is no array.
#
# tmux refuses `set -g 'status-left[x]' v` with "not an array:
# status-left[x]" (cmd-set-option.c:295-298 at tag 3.8), on an @ option too,
# with every flag (-q and -u included), and sets nothing. show-options and
# formats read the option itself (options_to_string ignores the key of an
# option that is no array, options.c:720-756), so `show -g 'status-left[0]'`
# prints status-left's value under the name as typed. psmux accepted the set,
# kept the value as text under the name `status-left[x]` (listed by `show
# -g`), and show and formats read that text back.
#
# Each expectation is what tmux 3.8 (`-f /dev/null`, WSL) printed or did for
# the same commands:
#   [1] set refuses an element of an option that is no array, on every flag
#       and scope, and nothing is stored
#   [2] the names around it keep working: an unknown name stays an invalid
#       option (which -q silences), a subscript options_parse refuses is an
#       invalid option, and array and hook elements are set
#   [3] show and formats read the option itself
#   [4] the same line in the config file, and run from a hook, sets nothing
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_na_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
$script:NS = ""
# Output (stdout and stderr, one line) and the exit code of one command.
function P { (& $PSMUX -L $script:NS @args 2>&1 | ForEach-Object { "$_" }) -join "|" }
function Rc { & $PSMUX -L $script:NS @args 2>&1 | Out-Null; $LASTEXITCODE }
function New-Ns($conf) {
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Encoding ascii -Value $conf
    $script:NS = "na_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:StartOutput = (& $PSMUX -L $script:NS new-session -d -s s 2>&1 | ForEach-Object { "$_" }) -join "|"
    Start-Sleep -Milliseconds 1500
}
function Kill-Ns { if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 500 } }
# The show -g lines whose name holds a `[`, other than the arrays'.
function Stray {
    # Hooks are arrays too; how psmux lists a hook element set with
    # set-option is its own matter, so they are left out here.
    $arrays = '^(command-alias|codepoint-widths|pane-colours|status-format|terminal-features|terminal-overrides|update-environment|user-keys|after-[a-z-]+)\['
    ((& $PSMUX -L $script:NS show -g 2>&1 | ForEach-Object { "$_" } | Where-Object { $_ -match '^\S*\[' -and $_ -notmatch $arrays }) -join "|")
}

try {
    Write-Host "[1] set refuses an element of an option that is no array" -ForegroundColor Cyan
    New-Ns @('set -g @u hello')
    $left = P show -gv status-left
    foreach ($case in @(
            @('set', '-g', 'status-left[x]', 'v'),
            @('set', '-g', 'status-left[0]', 'v'),
            @('set', '-ga', 'status-left[0]', 'v'),
            @('set', '-gu', 'status-left[0]'),
            @('set', '-go', 'status-left[0]', 'v'),
            @('set', '-gq', 'status-left[0]', 'v'),
            @('set', '-s', 'escape-time[0]', '5'),
            @('setw', '-g', 'mode-keys[0]', 'vi'),
            @('set', '-g', 'visual-silence[0]', 'on'),
            @('set', '-g', '@user[0]', 'v'),
            @('set', '-g', '@u[0]', 'v'),
            @('set', '-gu', '@u[0]'))) {
        $out = P @case
        $rc = $LASTEXITCODE
        $label = $case -join ' '
        if ($rc -ne 0 -and $out -match [regex]::Escape("not an array: $($case[2])")) {
            Write-Pass "$label : rc=$rc '$out'"
        } else {
            Write-Fail "$label : rc=$rc '$out'"
        }
    }
    Check "status-left is unchanged" (P show -gv status-left) $left
    Check "escape-time is unchanged" (P show -sv escape-time) "10"
    Check "mode-keys is unchanged" (P show -gwv mode-keys) "emacs"
    Check "@u is unchanged" (P show -gv '@u') "hello"
    Check "show -g lists no element of an option that is no array" (Stray) ""
    Kill-Ns

    Write-Host "[2] the names around it" -ForegroundColor Cyan
    New-Ns @()
    $out = P set -g 'nosuch[0]' v
    if ($LASTEXITCODE -ne 0 -and $out -match 'invalid option: nosuch\[0\]') { Write-Pass "nosuch[0] is an invalid option: '$out'" } else { Write-Fail "nosuch[0] rc=$LASTEXITCODE '$out'" }
    Check "set -gq nosuch[0] is silent" ((P set -gq 'nosuch[0]' v) + " rc=$LASTEXITCODE") " rc=0"
    foreach ($name in @('status-left[]', 'status-left[0]x', '@u[]')) {
        $out = P set -g $name v
        if ($LASTEXITCODE -ne 0 -and $out -match [regex]::Escape("invalid option: $name")) { Write-Pass "$name is an invalid option: '$out'" } else { Write-Fail "$name rc=$LASTEXITCODE '$out'" }
    }
    Check "set -g status-format[1]" (Rc set -g 'status-format[1]' ok) "0"
    Check "status-format[1] is set" (P show -gv 'status-format[1]') "ok"
    Check "set -g command-alias[9]" (Rc set -g 'command-alias[9]' 'zz=list-keys') "0"
    Check "set-hook -g after-new-window[5]" (Rc set-hook -g 'after-new-window[5]' 'set -g @h5 yes') "0"
    Check "set -g after-new-window[6]" (Rc set -g 'after-new-window[6]' 'set -g @h6 yes') "0"
    Check "show -g lists no element of an option that is no array" (Stray) ""
    Kill-Ns

    Write-Host "[3] show and formats read the option itself" -ForegroundColor Cyan
    New-Ns @('set -g status-left "left "', 'set -g @u hello')
    # The line `show -g status-left` prints, under the name as typed (tmux
    # quotes the value there; how psmux quotes a single option is the same
    # with or without the key).
    $line = P show -g status-left
    Check "show -g status-left is set" ($line -match 'left') $true
    Check "show -g status-left[0]" (P show -g 'status-left[0]') ($line -replace '^status-left', 'status-left[0]')
    Check "show -g status-left[y]" (P show -g 'status-left[y]') ($line -replace '^status-left', 'status-left[y]')
    Check "show -gv status-left[0]" (P show -gv 'status-left[0]') 'left '
    Check "show -s escape-time[3]" (P show -s 'escape-time[3]') 'escape-time[3] 10'
    Check "show -gw mode-keys[0]" (P show -gw 'mode-keys[0]') 'mode-keys[0] emacs'
    Check "show -g @u[0]" (P show -g '@u[0]') '@u[0] hello'
    Check "show -gv @u[0]" (P show -gv '@u[0]') 'hello'
    $out = P show -g '@nope[0]'
    if ($LASTEXITCODE -ne 0 -and $out -match 'invalid option: @nope\[0\]') { Write-Pass "@nope[0] unset is an invalid option: '$out'" } else { Write-Fail "@nope[0] rc=$LASTEXITCODE '$out'" }
    Check "#{status-left[x]}|#{escape-time[0]}|#{@u[0]}|#{mode-keys[1]}" (P display -p '#{status-left[x]}|#{escape-time[0]}|#{@u[0]}|#{mode-keys[1]}|') 'left |10|hello|emacs|'
    Kill-Ns

    Write-Host "[4] a config line and a hook set nothing" -ForegroundColor Cyan
    New-Ns @("set -g 'status-left[x]' fromconf", "set -g '@u[0]' fromconf", 'set -g @after yes', "set-hook -g after-new-window `"set -g 'status-left[y]' fromhook`"")
    $warnings = $script:StartOutput + "|" + ((Get-ChildItem $TempHome -Recurse -Filter config-warnings.log -EA SilentlyContinue | ForEach-Object { Get-Content $_.FullName }) -join "|")
    foreach ($name in @('status-left[x]', '@u[0]')) {
        if ($warnings -match [regex]::Escape("not an array: $name")) { Write-Pass "the config line on $name is reported" } else { Write-Fail "no warning for $name : '$warnings'" }
    }
    Check "the lines after it still run" (P show -gv '@after') "yes"
    Check "status-left keeps its default" (P show -gv status-left) "[#S] "
    & $PSMUX -L $script:NS new-window -d 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Check "show -g lists nothing from the config or the hook" (Stray) ""
    Kill-Ns

    # set-hook is set-option in tmux (cmd-set-option.c:65), so the same
    # check holds there: `set-hook -g status-left[0] cmd` is "not an array"
    # and no hook is kept, while a hook element is set.
    Write-Host "[5] set-hook refuses an element of an option that is no array" -ForegroundColor Cyan
    New-Ns @("set-hook -g 'status-right[c]' 'display c'", 'set -g @after yes')
    foreach ($case in @(
            @('set-hook', '-g', 'status-left[0]', 'display x'),
            @('set-hook', '-g', '@u[0]', 'display x'),
            @('set-hook', '-ga', 'escape-time[1]', 'display x'),
            @('set-hook', '-gu', '@u[0]'))) {
        $out = P @case
        $rc = $LASTEXITCODE
        $label = $case -join ' '
        if ($rc -ne 0 -and $out -match [regex]::Escape("not an array: $($case[2])")) {
            Write-Pass "$label : rc=$rc '$out'"
        } else {
            Write-Fail "$label : rc=$rc '$out'"
        }
    }
    $cc = (("set-hook -g 'status-left[cc]' 'display x'`n" | & $PSMUX -L $script:NS -C attach -t s 2>&1 | ForEach-Object { "$_" }) -join "|")
    if ($cc -match 'not an array: status-left\[cc\]\|%error') { Write-Pass "control mode set-hook status-left[cc] is %error" } else { Write-Fail "control mode set-hook: '$cc'" }
    Check "set-hook -g pane-exited[3] is set" (Rc set-hook -g 'pane-exited[3]' 'display y') "0"
    $hooks = (& $PSMUX -L $script:NS show-hooks -g 2>&1 | ForEach-Object { "$_" })
    Check "show-hooks lists no element of an option that is no array" (($hooks | Where-Object { $_ -match '^(status-left|status-right|escape-time|@u)\[' }) -join "|") ""
    Check "show-hooks lists pane-exited[3]" ([bool]($hooks | Where-Object { $_ -match '^pane-exited\[3\]' })) $true
    Kill-Ns
} finally {
    Kill-Ns
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host ""
Write-Host "Passed: $script:TestsPassed  Failed: $script:TestsFailed"
exit $script:TestsFailed
