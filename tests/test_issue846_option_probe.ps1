# Issue #846: option reads answer what tmux answers, so a program can probe.
#
# tmux refuses an option name its table does not know unless it starts with
# `@`: set-option and show-options fail with "invalid option: NAME" at exit 1
# (cmd-set-option.c:269, cmd-show-options.c:117), and -q turns that into a
# silent success that sets nothing. extended-keys is a real tmux option,
# server scope, choices off/on/always, default off (options-table.c:402);
# extended-keys-format is csi-u/xterm, default xterm (:411). A choice outside
# the list is "unknown value: X" (options.c:1229).
#
# psmux stored any name and read it back: `set -g extended-keys on` made
# `show -gv extended-keys` print `on` (empty before), `set -g zzzbogusprobe 1`
# landed in the environment map where `set -gu` could not clear it, and
# `#{allow-passthrough}` expanded empty while `show -gv` said `off`.
#
# Only the CLI is used, under a namespace and data dir of its own.
# Set PSMUX_BIN (or PSMUX_TEST_BIN) to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$NS = "i846-" + [guid]::NewGuid().ToString('N').Substring(0, 6)

function PX {
    $o = (& $PSMUX -L $NS @args 2>&1 | ForEach-Object { "$_" }) -join ' '
    return @{ out = $o.Trim(); rc = $LASTEXITCODE }
}
# One command against what tmux gives: exit status, and output (exact when
# $exact, else a substring).
function Expect($label, $r, [int]$rc, [string]$out, [switch]$exact) {
    $ok = $r.rc -eq $rc -and $(if ($exact) { $r.out -ceq $out } elseif ($out) { $r.out -match [regex]::Escape($out) } else { -not $r.out })
    if ($ok) { Write-Pass "$label -> rc=$($r.rc) [$($r.out)]" }
    else { Write-Fail "$label -> rc=$($r.rc) out=[$($r.out)]; tmux: rc=$rc [$out]" }
}

try {
    & $PSMUX -L $NS new-session -d -s s -x 80 -y 20 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 150
        & $PSMUX -L $NS has-session -t s 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { break }
    }

    Write-Head "1. extended-keys is a real option with tmux's defaults"
    Expect "show -gv extended-keys (unset)" (PX show -gv extended-keys) 0 'off' -exact
    Expect "show -gv extended-keys-format (unset)" (PX show -gv extended-keys-format) 0 'xterm' -exact
    Expect "show -g extended-keys" (PX show -g extended-keys) 0 'extended-keys off' -exact
    Expect "#{extended-keys}" (PX display -p '#{extended-keys}') 0 'off' -exact
    Expect "set -g extended-keys on" (PX set -g extended-keys on) 0 ''
    Expect "show -gv extended-keys after set" (PX show -gv extended-keys) 0 'on' -exact
    Expect "set -s extended-keys always" (PX set -s extended-keys always) 0 ''
    Expect "show -sv extended-keys" (PX show -sv extended-keys) 0 'always' -exact
    Expect "set -g extended-keys bogus" (PX set -g extended-keys bogus) 1 'unknown value: bogus'
    Expect "value unchanged after refusal" (PX show -gv extended-keys) 0 'always' -exact
    Expect "set -g extended-keys-format csi-u" (PX set -g extended-keys-format csi-u) 0 ''
    Expect "#{extended-keys-format}" (PX display -p '#{extended-keys-format}') 0 'csi-u' -exact
    Expect "set -gu extended-keys" (PX set -gu extended-keys) 0 ''
    Expect "back to off after -u" (PX show -gv extended-keys) 0 'off' -exact

    Write-Head "2. an unknown plain name is refused, and so stored nowhere"
    Expect "set -g zzzbogusprobe 1" (PX set -g zzzbogusprobe 1) 1 'invalid option: zzzbogusprobe'
    Expect "show -gv zzzbogusprobe" (PX show -gv zzzbogusprobe) 1 'invalid option: zzzbogusprobe'
    Expect "set -gu zzzbogusprobe" (PX set -gu zzzbogusprobe) 1 'invalid option: zzzbogusprobe'
    Expect "not in the environment" (PX show-environment -g zzzbogusprobe) 1 'unknown variable'
    $envs = (PX show-environment).out
    if ($envs -notmatch 'zzzbogusprobe') { Write-Pass "show-environment has no zzzbogusprobe" } else { Write-Fail "show-environment lists zzzbogusprobe: $envs" }

    Write-Head "3. an unknown hyphenated name is refused"
    Expect "set -g bogus-hyphen-probe x" (PX set -g bogus-hyphen-probe x) 1 'invalid option: bogus-hyphen-probe'
    Expect "show -gv bogus-hyphen-probe" (PX show -gv bogus-hyphen-probe) 1 'invalid option: bogus-hyphen-probe'
    Expect "setw -g bogus-hyphen-probe x" (PX setw -g bogus-hyphen-probe x) 1 'invalid option: bogus-hyphen-probe'
    $all = (PX show -g).out
    if ($all -notmatch 'bogus-hyphen-probe') { Write-Pass "show -g does not list it" } else { Write-Fail "show -g lists bogus-hyphen-probe" }

    Write-Head "4. -q: silent success that sets nothing"
    Expect "set -gq zzzbogusprobe 1" (PX set -gq zzzbogusprobe 1) 0 ''
    Expect "set -gq bogus-hyphen-probe x" (PX set -gq bogus-hyphen-probe x) 0 ''
    Expect "show -gqv bogus-hyphen-probe" (PX show -gqv bogus-hyphen-probe) 0 ''
    Expect "still invalid without -q" (PX show -gv bogus-hyphen-probe) 1 'invalid option'

    Write-Head "5. #{allow-passthrough} agrees with show"
    Expect "show -gv allow-passthrough" (PX show -gv allow-passthrough) 0 'off' -exact
    Expect "#{allow-passthrough}" (PX display -p '#{allow-passthrough}') 0 'off' -exact
    PX set -g allow-passthrough on | Out-Null
    Expect "#{allow-passthrough} after set" (PX display -p '#{allow-passthrough}') 0 'on' -exact
    foreach ($n in 'set-clipboard', 'alternate-screen', 'repeat-time', 'status-left-length', 'key-table', 'allow-rename') {
        $s = (PX show -gv $n).out
        $f = (PX display -p "#{$n}").out
        if ($s -ceq $f -and $s) { Write-Pass "#{$n} = show -gv $n = '$s'" } else { Write-Fail "#{$n} '$f' but show -gv '$s'" }
    }

    Write-Head "6. what must keep working"
    Expect "set -g @probe v" (PX set -g '@probe' v) 0 ''
    Expect "show -gv @probe" (PX show -gv '@probe') 0 'v' -exact
    Expect "show -gv @never-set" (PX show -gv '@never-set') 1 'invalid option: @never-set'
    # A tmux option psmux has no setter for is accepted, as tmux accepts it.
    Expect "set -g visual-silence on" (PX set -g visual-silence on) 0 ''
    Expect "show -gv visual-silence" (PX show -gv visual-silence) 0 'on' -exact
    Expect "set -s backspace C-?" (PX set -s backspace 'C-?') 0 ''
    Expect "backspace not in the environment" (PX show-environment -g backspace) 1 'unknown variable'
    Expect "set -g status-format[1] x" (PX set -g 'status-format[1]' x) 0 ''
    Expect "set -g mouse off" (PX set -g mouse off) 0 ''
    Expect "show -gv mouse" (PX show -gv mouse) 0 'off' -exact
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
