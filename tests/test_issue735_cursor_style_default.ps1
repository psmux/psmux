# Issue #735: psmux asserts a cursor shape on every attach and defaults to
# `bar`, so attaching replaces the cursor shape the terminal was configured
# with. tmux defaults `cursor-style` to `default` and never asserts a shape of
# its own (options-table.c:336-341, tty.c:332-386, tty.c:816-825, tag 3.7c
# `e476c123`).
#
# What this checks, from outside the process:
#   the option reports `default` when nothing set it, and `cursor-blink` off
#   a shape can still be set, read back and cleared again
#   `psmux --help` states the same defaults as the catalog and lists `default`
#
# What it cannot check: the bytes psmux writes to the real terminal. Attaching
# needs a console, and the write happens before anything can be piped. The
# attach path is covered byte for byte by the unit test over
# `apply_cursor_style` (tests-rs/test_issue735_cursor_style_default.rs).
#
# Before the fix: 4 passed, 4 failed. After: 8 passed, 0 failed.

$ErrorActionPreference = "Continue"
$env:PSMUX_NO_WARM = "1"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i735"
$SESS = "c1"
# An empty config, passed with -f, so the session reports the option's own
# default and not whatever the person running this has in their psmux.conf.
# Without it this suite measures the tester: a `set -g cursor-style default`
# line of their own makes the first check pass on any build.
$CONF = Join-Path $env:TEMP "psmux_i735_empty.conf"
Set-Content -Path $CONF -Value "# empty on purpose, see the comment above" -Encoding ASCII

$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function P { & $PSMUX -f $CONF -L $NS @args 2>&1 }

function Get-Opt([string]$Name) {
    $line = (P show-options -g -t $SESS | Out-String) -split "`r?`n" |
        Where-Object { $_ -match ("^" + [regex]::Escape($Name) + "\s") } |
        Select-Object -First 1
    if (-not $line) { return "" }
    return ($line -replace ("^" + [regex]::Escape($Name) + "\s+"), "").Trim()
}

function Check([string]$What, [string]$Got, [string]$Want) {
    if ($Got -eq $Want) { Write-Pass "$What is [$Want]" }
    else { Write-Fail "$What is [$Got], wanted [$Want]" }
}

Write-Host "binary: $PSMUX" -ForegroundColor Cyan
Write-Host ("version: " + (& $PSMUX -V)) -ForegroundColor Cyan

P kill-server | Out-Null
Start-Sleep -Milliseconds 300
P new-session -d -s $SESS -x 80 -y 24 | Out-Null
$up = $false
for ($i = 0; $i -lt 40; $i++) {
    if ((P list-sessions | Out-String) -match $SESS) { $up = $true; break }
    Start-Sleep -Milliseconds 250
}
if (-not $up) { Write-Host "FATAL: no session" -ForegroundColor Red; exit 1 }

# ---- the defaults, with nothing set ----
Check "cursor-style with nothing set" (Get-Opt "cursor-style") "default"
# Off, so a bare block/underline/bar is the steady shape tmux means by the
# word. This is the one incompatible change: psmux defaulted it to on.
Check "cursor-blink with nothing set" (Get-Opt "cursor-blink") "off"

# ---- a shape can still be asked for, and given up again ----
P set -g cursor-style block | Out-Null
Start-Sleep -Milliseconds 300
Check "cursor-style after setting block" (Get-Opt "cursor-style") "block"

P set -g cursor-style default | Out-Null
Start-Sleep -Milliseconds 300
Check "cursor-style set back to default" (Get-Opt "cursor-style") "default"

# ---- tmux spells the blink into the value ----
# tmux has no cursor-blink option: its cursor-style carries the blink
# (options-table.c:62-65). A config written for tmux has to name a shape psmux
# understands. The value has always been STORED, because the option is never
# validated; what it resolves to is the unit test's subject
# (tests-rs/test_issue735_cursor_style_default.rs), since the DECSCUSR code
# goes to the real terminal and nothing here can read it back. This only pins
# the round trip, so the value stays spelled the way the config wrote it.
P set -g cursor-blink on | Out-Null
P set -g cursor-style blinking-bar | Out-Null
Start-Sleep -Milliseconds 300
Check "cursor-style keeps the tmux spelling blinking-bar" (Get-Opt "cursor-style") "blinking-bar"

P set -g cursor-style default | Out-Null
Start-Sleep -Milliseconds 300

# ---- the CLI help says what the catalog says ----
# `src/cli.rs` prints its help from one `println!(r#"..."#)`, so running the
# binary is the only way to read it. Each entry wraps over several lines, so
# the whole entry is read, not the first line of it.
$help = (& $PSMUX --help 2>&1 | Out-String) -split "`r?`n"

function Get-HelpEntry([string]$Name) {
    $i = 0
    while ($i -lt $help.Count -and $help[$i] -notmatch ("^\s+" + [regex]::Escape($Name) + "\s")) { $i++ }
    if ($i -ge $help.Count) { return "" }
    $entry = $help[$i]
    $i++
    # Continuation lines are indented past the description column and start no
    # option of their own.
    while ($i -lt $help.Count -and $help[$i] -match "^\s{25,}\S") { $entry += " " + $help[$i].Trim(); $i++ }
    return $entry
}

$styleEntry = Get-HelpEntry "cursor-style"
foreach ($want in @("blinking-bar", "default: default")) {
    if ($styleEntry -match [regex]::Escape($want)) {
        Write-Pass "psmux --help for cursor-style mentions $want"
    } else {
        Write-Fail "psmux --help for cursor-style omits ${want}: [$styleEntry]"
    }
}

$blinkEntry = Get-HelpEntry "cursor-blink"
if ($blinkEntry -match "default: off") {
    Write-Pass "psmux --help states cursor-blink defaults to off"
} else {
    Write-Fail "psmux --help states the wrong cursor-blink default: [$blinkEntry]"
}

P kill-server | Out-Null
Start-Sleep -Milliseconds 300

Write-Host ""
Write-Host ("RESULT: {0} passed, {1} failed" -f $script:Pass, $script:Fail) -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 }
exit 0
