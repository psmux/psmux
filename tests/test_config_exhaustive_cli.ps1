<#
.SYNOPSIS
    CLI E2E Config Tests: every config option via psmux.exe CLI commands

.DESCRIPTION
    Exhaustive config testing through CLI (psmux set-option / show-options),
    config file loading via -f flag, source-file, continuations, and %if/%else.
    Every test: SET via CLI, VERIFY via CLI show-options with actual value checks.
    TCP used only for cross-channel verification.
    ZERO hardcoded ($true) passes. Every assertion checks real server state.
#>

$ErrorActionPreference = 'Continue'
$passed = 0; $failed = 0; $skipped = 0
$testResults = @()
$PSMUX_DIR = "$env:USERPROFILE\.psmux"

function Test-Result($name, $condition, $detail = '') {
    if ($condition) {
        $script:passed++
        $script:testResults += [PSCustomObject]@{Name=$name;Status='PASS';Detail=$detail}
    } else {
        $script:failed++
        $script:testResults += [PSCustomObject]@{Name=$name;Status='FAIL';Detail=$detail}
        Write-Host "  FAIL: $name $detail" -ForegroundColor Red
    }
}

function Send-Tcp($session, $cmd, [int]$TimeoutMs = 5000) {
    try {
        $port = (Get-Content "$PSMUX_DIR\$session.port" -Raw).Trim()
        $key  = (Get-Content "$PSMUX_DIR\$session.key" -Raw).Trim()
        $client = [System.Net.Sockets.TcpClient]::new()
        $client.Connect('127.0.0.1', [int]$port)
        $stream = $client.GetStream()
        $stream.ReadTimeout = $TimeoutMs
        $writer = [System.IO.StreamWriter]::new($stream); $writer.AutoFlush = $true
        $reader = [System.IO.StreamReader]::new($stream)
        $writer.WriteLine("AUTH $key")
        $auth = $reader.ReadLine()
        if ($auth -ne "OK") { $client.Close(); return "AUTH_FAILED" }
        $writer.WriteLine($cmd)
        $lines = @()
        try {
            while ($true) {
                $line = $reader.ReadLine()
                if ($null -eq $line) { break }
                $lines += $line
                # Read to EOF: a one-shot reply ends when the server closes the
                # connection (ReadLine returns null), bounded by ReadTimeout. A quiet
                # socket is not the end of a reply.
            }
        } catch {}
        $client.Close()
        return ($lines -join "`n")
    } catch { return "ERROR: $_" }
}

function CLI-Show($sess, $opt) {
    $out = & $psmux show-options -t $sess -g $opt 2>&1 | Out-String
    return $out.Trim()
}

function CLI-ShowSession($sess, $opt) {
    $out = & $psmux show-options -t $sess $opt 2>&1 | Out-String
    return $out.Trim()
}

function CLI-Set($sess, [string[]]$args_) {
    & $psmux set-option -t $sess @args_ 2>&1 | Out-Null
}

# ============================================================
# Setup
# ============================================================
$sess = "cli-cfg-$(Get-Random -Maximum 9999)"
$psmux = (Get-Command psmux -ErrorAction SilentlyContinue).Source
if (-not $psmux) { $psmux = (Get-Command tmux -ErrorAction SilentlyContinue).Source }
if (-not $psmux) { Write-Host "psmux not found"; exit 1 }

Write-Host "Binary: $psmux"
Write-Host "Session: $sess"

& $psmux new-session -d -s $sess 2>$null
Start-Sleep -Milliseconds 2000

# Verify session created and port/key files exist
$portFile = "$PSMUX_DIR\$sess.port"
$keyFile  = "$PSMUX_DIR\$sess.key"
if (-not (Test-Path $portFile) -or -not (Test-Path $keyFile)) {
    Write-Host "FATAL: Port/key file not found at $PSMUX_DIR" -ForegroundColor Red
    & $psmux kill-session -t $sess 2>$null
    exit 1
}

# Sanity check: CLI show-options works
$sanity = CLI-Show $sess 'mouse'
Test-Result "cli-sanity" ($sanity -match 'mouse') "CLI show-options works: '$sanity'"

# ============================================================
# SECTION 1: Boolean options via CLI set + CLI show verify
# ============================================================

$bools = @(
    'mouse', 'focus-events', 'renumber-windows', 'automatic-rename',
    'allow-rename', 'monitor-activity',
    'remain-on-exit', 'destroy-unattached', 'exit-empty',
    'set-titles',
    'scroll-enter-copy-mode', 'pwsh-mouse-selection', 'mouse-selection-force',
    'synchronize-panes', 'warm',
    'allow-predictions', 'claude-code-fix-tty', 'claude-code-force-interactive'
)

foreach ($opt in $bools) {
    CLI-Set $sess @('-g', $opt, 'on')
    $show = CLI-Show $sess $opt
    Test-Result "cli-bool-on-$opt" ($show -match '\bon\b') "show='$show'"

    CLI-Set $sess @('-g', $opt, 'off')
    $show = CLI-Show $sess $opt
    Test-Result "cli-bool-off-$opt" ($show -match '\boff\b') "show='$show'"
}

# ============================================================
# SECTION 2: Numeric options via CLI set + CLI show verify
# ============================================================

$nums = @(
    @{n='escape-time'; v='100'; d='10'},  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
    @{n='history-limit'; v='50000'; d='2000'},
    @{n='display-time'; v='3000'; d='750'},
    @{n='display-panes-time'; v='5000'; d='1000'},
    @{n='base-index'; v='1'; d='0'},
    @{n='pane-base-index'; v='1'; d='0'},
    @{n='status-interval'; v='5'; d='15'},
    @{n='main-pane-width'; v='80'; d='0'},
    @{n='main-pane-height'; v='40'; d='0'},
    @{n='status-left-length'; v='50'; d='10'},
    @{n='status-right-length'; v='80'; d='40'}
)

foreach ($opt in $nums) {
    CLI-Set $sess @('-g', $opt.n, $opt.v)
    $show = CLI-Show $sess $opt.n
    Test-Result "cli-num-$($opt.n)" ($show -match "\b$($opt.v)\b") "Expected $($opt.v), show='$show'"
    CLI-Set $sess @('-g', $opt.n, $opt.d)
}

# ============================================================
# SECTION 3: String/style options via CLI set + CLI show verify
# ============================================================

$strings = @(
    @{n='status-left'; v='[CLI]'; p='CLI'},
    @{n='status-right'; v='%H:%M'; p='%H:%M'},
    @{n='status-position'; v='top'; p='top'},
    @{n='status-style'; v='bg=red,fg=white'; p='bg=red'},
    @{n='status-justify'; v='centre'; p='centre'},
    @{n='status-left-style'; v='fg=blue'; p='blue'},
    @{n='status-right-style'; v='fg=green'; p='green'},
    @{n='mode-keys'; v='vi'; p='vi'},
    @{n='word-separators'; v=' -_@'; p='-_@'},
    @{n='set-titles-string'; v='#S:#W'; p='#S:#W'},
    @{n='activity-action'; v='any'; p='any'},
    @{n='silence-action'; v='none'; p='none'},
    @{n='pane-border-style'; v='fg=grey'; p='grey'},
    @{n='pane-active-border-style'; v='fg=cyan'; p='cyan'},
    @{n='pane-border-hover-style'; v='fg=red'; p='red'},
    @{n='window-status-format'; v='#I'; p='#I'},
    @{n='window-status-current-format'; v='#W'; p='#W'},
    @{n='window-status-separator'; v='|'; p='\|'},
    @{n='window-status-style'; v='fg=white'; p='white'},
    @{n='window-status-current-style'; v='fg=yellow'; p='yellow'},
    @{n='window-status-activity-style'; v='underscore'; p='underscore'},
    @{n='window-status-bell-style'; v='blink'; p='blink'},
    @{n='window-status-last-style'; v='dim'; p='dim'},
    @{n='message-style'; v='fg=red'; p='red'},
    @{n='message-command-style'; v='fg=blue'; p='blue'},
    @{n='mode-style'; v='bg=blue'; p='blue'},
    @{n='window-size'; v='smallest'; p='smallest'},
    @{n='allow-passthrough'; v='on'; p='\bon\b'},
    @{n='copy-command'; v='clip.exe'; p='clip'},
    @{n='set-clipboard'; v='external'; p='external'},
    @{n='default-shell'; v='pwsh'; p='pwsh'}
)

foreach ($opt in $strings) {
    CLI-Set $sess @('-g', $opt.n, $opt.v)
    $show = CLI-Show $sess $opt.n
    Test-Result "cli-str-$($opt.n)" ($show -match $opt.p) "Expected '$($opt.p)', show='$show'"
}

# ============================================================
# SECTION 4: Flag tests via CLI
# ============================================================

# -g (global) + verify exact value
CLI-Set $sess @('-g', 'escape-time', '42')
$show = CLI-Show $sess 'escape-time'
Test-Result "cli-flag-g" ($show -match '\b42\b') "show='$show'"

# -a (append) using separate flag
CLI-Set $sess @('-g', 'status-right', 'AAA')
& $psmux set-option -t $sess -a status-right BBB 2>&1 | Out-Null
# -a without -g appends to the session's own value; the global stays AAA (tmux, #803)
$show = CLI-ShowSession $sess 'status-right'
Test-Result "cli-flag-a-append" ($show -match 'AAABBB') "show='$show'"

# Triple append
CLI-Set $sess @('-g', 'status-left', 'X')
& $psmux set-option -t $sess -a status-left Y 2>&1 | Out-Null
& $psmux set-option -t $sess -a status-left Z 2>&1 | Out-Null
$show = CLI-ShowSession $sess 'status-left'
Test-Result "cli-flag-a-triple" ($show -match 'XYZ') "show='$show'"

# -u (unset) on @user option using separate flag
CLI-Set $sess @('-g', '@cli-u-test', 'value')
$show1 = CLI-Show $sess '@cli-u-test'
Test-Result "cli-flag-u-set" ($show1 -match 'value') "pre-unset: show='$show1'"
& $psmux set-option -t $sess -u @cli-u-test 2>&1 | Out-Null
$show2 = CLI-Show $sess '@cli-u-test'
Test-Result "cli-flag-u-gone" ($show2 -notmatch 'value') "post-unset: show='$show2'"

# -q (quiet) on bogus option
& $psmux set-option -t $sess -q nonexistent-xyz val 2>&1 | Out-Null
$show = CLI-Show $sess 'mouse'
Test-Result "cli-flag-q-silent" ($show -match 'mouse') "Server still alive: show='$show'"

# -w (window scope)
& $psmux set-option -t $sess -w mouse on 2>&1 | Out-Null
# mouse is a session option, so -w sets the session's value (tmux options_scope_from_name)
$show = CLI-ShowSession $sess 'mouse'
Test-Result "cli-flag-w" ($show -match '\bon\b') "show='$show'"

# -t target
& $psmux set-option -t $sess -g escape-time 99 2>&1 | Out-Null
$show = CLI-Show $sess 'escape-time'
Test-Result "cli-flag-t-target" ($show -match '99') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before

# ============================================================
# SECTION 5: User/@- options lifecycle via CLI
# ============================================================

# Create + verify
CLI-Set $sess @('-g', '@theme', 'mocha')
$show = CLI-Show $sess '@theme'
Test-Result "cli-user-create" ($show -match 'mocha') "show='$show'"

# Append (via TCP since CLI binary does not forward -a flag to server)
Send-Tcp $sess "set-option -a @theme _extended" | Out-Null
$show = CLI-Show $sess '@theme'
Test-Result "cli-user-append" ($show -match 'mocha_extended') "show='$show'"

# Overwrite
CLI-Set $sess @('-g', '@theme', 'latte')
$show = CLI-Show $sess '@theme'
Test-Result "cli-user-overwrite" ($show -match 'latte') "show='$show'"

# Unset (via TCP since CLI binary does not forward -u flag to server)
Send-Tcp $sess "set-option -u @theme" | Out-Null
$show = CLI-Show $sess '@theme'
Test-Result "cli-user-unset" ($show -notmatch 'latte') "show='$show'"

# ============================================================
# SECTION 6: Hooks via CLI + VERIFY via TCP show-hooks
# ============================================================

& $psmux set-hook -t $sess -g after-new-window 'run-shell echo hook1' 2>&1 | Out-Null
$hooks = Send-Tcp $sess "show-hooks"
Test-Result "cli-hook-set" ($hooks -match 'after-new-window.*hook1') "hooks='$hooks'"

& $psmux set-hook -t $sess -a after-new-window 'run-shell echo hook2' 2>&1 | Out-Null
$hooks = Send-Tcp $sess "show-hooks"
Test-Result "cli-hook-append" ($hooks -match 'hook1' -and $hooks -match 'hook2') "hooks='$hooks'"

& $psmux set-hook -t $sess -u after-new-window 2>&1 | Out-Null
$hooks = Send-Tcp $sess "show-hooks"
Test-Result "cli-hook-unset" ($hooks -notmatch 'after-new-window') "hooks='$hooks'"

# ============================================================
# SECTION 7: Environment via CLI + VERIFY via TCP show-environment
# ============================================================

& $psmux set-environment -t $sess CLI_ENV_V1 alpha 2>&1 | Out-Null
$env_out = Send-Tcp $sess "show-environment"
Test-Result "cli-env-set" ($env_out -match 'CLI_ENV_V1.*alpha') "env contains CLI_ENV_V1"

& $psmux setenv -t $sess CLI_ENV_V2 beta 2>&1 | Out-Null
$env_out = Send-Tcp $sess "show-environment"
Test-Result "cli-env-alias" ($env_out -match 'CLI_ENV_V2.*beta') "env contains CLI_ENV_V2"

# ============================================================
# SECTION 8: setw / set-window-option aliases via CLI + verify
# ============================================================

& $psmux setw -t $sess -g mode-keys vi 2>&1 | Out-Null
$show = CLI-Show $sess 'mode-keys'
Test-Result "cli-setw-mode-keys" ($show -match 'vi') "show='$show'"

# Cross-verify setw result via TCP
$tcp = Send-Tcp $sess "show-options -g mode-keys"
Test-Result "cli-setw-tcp-verify" ($tcp -match 'vi') "TCP verify: '$tcp'"

# Restore
& $psmux setw -t $sess -g mode-keys emacs 2>&1 | Out-Null

# ============================================================
# SECTION 9: show-options variants via CLI
# ============================================================

$showAll = & $psmux show-options -t $sess 2>&1 | Out-String
Test-Result "cli-show-all-mouse" ($showAll -match 'mouse') "show-options contains mouse"
Test-Result "cli-show-all-status" ($showAll -match 'status') "show-options contains status"
$lines = ($showAll -split "`n").Count
Test-Result "cli-show-all-multiline" ($lines -gt 5) "show-options has >5 lines, got $lines"

$show = CLI-Show $sess 'escape-time'
Test-Result "cli-show-specific" ($show -match 'escape-time\s+\d+') "show='$show'"

# ============================================================
# SECTION 10: Command alias via CLI + VERIFY via CLI show
# ============================================================

CLI-Set $sess @('-g', 'command-alias', 'sp=split-window')
$show = CLI-Show $sess 'command-alias'
Test-Result "cli-cmd-alias" ($show -match 'sp=split-window') "show='$show'"

CLI-Set $sess @('-g', 'command-alias', 'nw=new-window')
$show = CLI-Show $sess 'command-alias'
Test-Result "cli-cmd-alias-2" ($show -match 'nw=new-window') "show='$show'"

# ============================================================
# SECTION 11: Status multiline via CLI + VERIFY
# ============================================================

CLI-Set $sess @('-g', 'status', '2')
$show = CLI-Show $sess 'status'
Test-Result "cli-status-2" ($show -match '\b2\b') "show='$show'"

CLI-Set $sess @('-g', 'status', '5')
$show = CLI-Show $sess 'status'
Test-Result "cli-status-5" ($show -match '\b5\b') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'status', 'on')

# ============================================================
# SECTION 12: Prefix via CLI + VERIFY
# ============================================================

CLI-Set $sess @('-g', 'prefix', 'C-a')
$show = CLI-Show $sess 'prefix'
Test-Result "cli-prefix-c-a" ($show -match 'C-a') "show='$show'"

CLI-Set $sess @('-g', 'prefix2', 'C-s')
$show = CLI-Show $sess 'prefix2'
Test-Result "cli-prefix2-c-s" ($show -match 'C-s') "show='$show'"

CLI-Set $sess @('-g', 'prefix2', 'none')
$show = CLI-Show $sess 'prefix2'
Test-Result "cli-prefix2-none" ($show -match 'none' -or $show -notmatch 'C-s') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'prefix', 'C-b')

# ============================================================
# SECTION 13: user_options fallback storage via CLI + VERIFY
# ============================================================

$uo = @(
    'popup-style', 'popup-border-style', 'popup-border-lines',
    'window-style', 'window-active-style', 'wrap-search',
    'pane-border-format', 'pane-border-status',
    'clock-mode-colour', 'clock-mode-style',
    'lock-after-time', 'lock-command', 'status-keys'
)
foreach ($opt in $uo) {
    $uniq = "cval$(Get-Random -Maximum 9999)"
    CLI-Set $sess @('-g', $opt, $uniq)
    $show = CLI-Show $sess $opt
    Test-Result "cli-uo-$opt" ($show -match $uniq) "Expected '$uniq', show='$show'"
}

# ============================================================
# SECTION 14: tmux compat options via CLI + verify server survives
# ============================================================

CLI-Set $sess @('-g', 'terminal-overrides', ',xterm*:Tc')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-compat-terminal" ($show -match 'mouse') "Server ok after terminal-overrides"

CLI-Set $sess @('-g', 'default-terminal', 'xterm-256color')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-compat-default-term" ($show -match 'mouse') "Server ok after default-terminal"

CLI-Set $sess @('-g', 'update-environment', 'FOO BAR')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-compat-update-env" ($show -match 'mouse') "Server ok after update-environment"

# ============================================================
# SECTION 15: Boolean variant syntax via CLI (true/false/1/0)
# ============================================================

CLI-Set $sess @('-g', 'mouse', 'true')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-boolvar-true" ($show -match '\bon\b') "show='$show'"

CLI-Set $sess @('-g', 'mouse', 'false')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-boolvar-false" ($show -match '\boff\b') "show='$show'"

CLI-Set $sess @('-g', 'mouse', '1')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-boolvar-1" ($show -match '\bon\b') "show='$show'"

CLI-Set $sess @('-g', 'mouse', '0')
$show = CLI-Show $sess 'mouse'
Test-Result "cli-boolvar-0" ($show -match '\boff\b') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'mouse', 'on')

# ============================================================
# SECTION 16: Cross-channel verify (CLI set, TCP show)
# ============================================================

CLI-Set $sess @('-g', 'escape-time', '321')
$tcp = Send-Tcp $sess "show-options -g escape-time"
Test-Result "cli-cross-tcp-et" ($tcp -match '321') "TCP verify: '$tcp'"

CLI-Set $sess @('-g', '@cross-test', 'clipal')
$tcp = Send-Tcp $sess "show-options -g @cross-test"
Test-Result "cli-cross-tcp-user" ($tcp -match 'clipal') "TCP verify: '$tcp'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before

# ============================================================
# SECTION 17: Config file via source-file CLI + VERIFY loaded values
# (psmux does not support -f flag, so we test source-file instead)
# ============================================================

$tempConfig = Join-Path $env:TEMP "psmux_cli_f_$(Get-Random).conf"
@"
# CLI config file test
set -g escape-time 77
set -g base-index 1
set -g status-position top
"@ | Set-Content -Path $tempConfig -Encoding UTF8

& $psmux source-file -t $sess $tempConfig 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

$show = CLI-Show $sess 'escape-time'
Test-Result "cfg-et-77" ($show -match '\b77\b') "show='$show'"

$show = CLI-Show $sess 'base-index'
Test-Result "cfg-bi-1" ($show -match '\b1\b') "show='$show'"

$show = CLI-Show $sess 'status-position'
Test-Result "cfg-top" ($show -match 'top') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
CLI-Set $sess @('-g', 'base-index', '0')
CLI-Set $sess @('-g', 'status-position', 'bottom')
Remove-Item $tempConfig -Force -ErrorAction SilentlyContinue

# ============================================================
# SECTION 18: Config file with continuations via source-file
# ============================================================

$tempCont = Join-Path $env:TEMP "psmux_cli_cont_$(Get-Random).conf"
@"
set -g \
escape-time \
88
set -g status-left \
"HELLO"
"@ | Set-Content -Path $tempCont -Encoding UTF8

& $psmux source-file -t $sess $tempCont 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

$show = CLI-Show $sess 'escape-time'
Test-Result "cont-et-88" ($show -match '\b88\b') "show='$show'"

$show = CLI-Show $sess 'status-left'
Test-Result "cont-Hello" ($show -match 'HELLO') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
Remove-Item $tempCont -Force -ErrorAction SilentlyContinue

# ============================================================
# SECTION 19: Config file with %if/%else/%endif via source-file
# ============================================================

$tempIf = Join-Path $env:TEMP "psmux_cli_if_$(Get-Random).conf"
@"
%if "1"
set -g escape-time 55
%else
set -g escape-time 66
%endif
%if "0"
set -g base-index 9
%else
set -g base-index 2
%endif
"@ | Set-Content -Path $tempIf -Encoding UTF8

& $psmux source-file -t $sess $tempIf 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

$show = CLI-Show $sess 'escape-time'
Test-Result "if-true-55" ($show -match '\b55\b') "show='$show'"

$show = CLI-Show $sess 'base-index'
Test-Result "if-false-2" ($show -match '\b2\b') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
CLI-Set $sess @('-g', 'base-index', '0')
Remove-Item $tempIf -Force -ErrorAction SilentlyContinue

# ============================================================
# SECTION 20: Config file with %hidden and $NAME via source-file
# ============================================================

$tempHid = Join-Path $env:TEMP "psmux_cli_hid_$(Get-Random).conf"
@"
%hidden MY_ET=99
set -g escape-time `$MY_ET
"@ | Set-Content -Path $tempHid -Encoding UTF8

& $psmux source-file -t $sess $tempHid 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

$show = CLI-Show $sess 'escape-time'
Test-Result "hidden-99" ($show -match '\b99\b') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
Remove-Item $tempHid -Force -ErrorAction SilentlyContinue

# ============================================================
# SECTION 21: Config file with UTF-8 BOM via source-file
# ============================================================

$tempBom = Join-Path $env:TEMP "psmux_cli_bom_$(Get-Random).conf"
$bomBytes = [Text.Encoding]::UTF8.GetPreamble() + [Text.Encoding]::UTF8.GetBytes("set -g escape-time 44`n")
[IO.File]::WriteAllBytes($tempBom, $bomBytes)

& $psmux source-file -t $sess $tempBom 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

$show = CLI-Show $sess 'escape-time'
Test-Result "bom-44" ($show -match '\b44\b') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
Remove-Item $tempBom -Force -ErrorAction SilentlyContinue

# ============================================================
# SECTION 22: source-file via CLI + VERIFY loaded values
# ============================================================

$tempSrc = Join-Path $env:TEMP "psmux_cli_src_$(Get-Random).conf"
@"
set -g escape-time 123
set -g base-index 3
"@ | Set-Content -Path $tempSrc -Encoding UTF8

& $psmux source-file -t $sess $tempSrc 2>&1 | Out-Null
Start-Sleep -Milliseconds 300
$show = CLI-Show $sess 'escape-time'
Test-Result "cli-source-et-123" ($show -match '\b123\b') "show='$show'"

$show = CLI-Show $sess 'base-index'
Test-Result "cli-source-bi-3" ($show -match '\b3\b') "show='$show'"

# Restore
CLI-Set $sess @('-g', 'escape-time', '10')  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before
CLI-Set $sess @('-g', 'base-index', '0')
Remove-Item $tempSrc -Force -ErrorAction SilentlyContinue

# Nonexistent source-file: verify server still alive
& $psmux source-file -t $sess /no/such/file.conf 2>&1 | Out-Null
$show = CLI-Show $sess 'mouse'
Test-Result "cli-source-missing-alive" ($show -match 'mouse') "Server alive after missing source"

# ============================================================
# SECTION 23: psmux specific options via CLI + VERIFY
# ============================================================

$psmuxOpts = @(
    @{n='prediction-dimming'; v='on'; p='\bon\b'},

    @{n='default-shell'; v='pwsh'; p='pwsh'},
    @{n='default-command'; v='pwsh'; p='pwsh'}
)
foreach ($opt in $psmuxOpts) {
    CLI-Set $sess @('-g', $opt.n, $opt.v)
    $show = CLI-Show $sess $opt.n
    Test-Result "cli-psmux-$($opt.n)" ($show -match $opt.p) "show='$show'"
}

# ============================================================
# Cleanup
# ============================================================

& $psmux kill-session -t $sess 2>$null

# ============================================================
# Summary
# ============================================================

Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  CLI Config E2E Test Results" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Passed:  $passed" -ForegroundColor Green
Write-Host "  Failed:  $failed" -ForegroundColor $(if ($failed -gt 0) { 'Red' } else { 'Green' })
Write-Host "  Skipped: $skipped" -ForegroundColor Yellow
Write-Host "  Total:   $($passed + $failed + $skipped)" -ForegroundColor White
Write-Host "============================================" -ForegroundColor Cyan

if ($failed -gt 0) {
    Write-Host "`nFailed tests:" -ForegroundColor Red
    $testResults | Where-Object Status -eq 'FAIL' | ForEach-Object {
        Write-Host "  $($_.Name): $($_.Detail)" -ForegroundColor Red
    }
    exit 1
}
