<#
.SYNOPSIS
    TCP Config Tests: every config option via authenticated TCP to psmux server

.DESCRIPTION
    Exhaustive config option testing through raw TCP socket with AUTH.
    Every test: SET value via TCP, VERIFY value via TCP show-options/show-hooks/show-environment.
    ZERO hardcoded ($true) passes. Every assertion checks actual server state.
    Uses separate flags (-g -a not -ga) because the TCP server's interactive
    path matches exact flag tokens.
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
        # AUTH handshake
        $writer.WriteLine("AUTH $key")
        $auth = $reader.ReadLine()
        if ($auth -ne "OK") { $client.Close(); return "AUTH_FAILED" }
        # Send command
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

# Setup
$sess = "tcp-cfg-$(Get-Random -Maximum 9999)"
$psmux = (Get-Command psmux -ErrorAction SilentlyContinue).Source
if (-not $psmux) { $psmux = (Get-Command tmux -ErrorAction SilentlyContinue).Source }
if (-not $psmux) { Write-Host "psmux not found"; exit 1 }

Write-Host "Binary: $psmux"
Write-Host "Session: $sess"

& $psmux new-session -d -s $sess 2>$null
Start-Sleep -Milliseconds 2000

$portFile = "$PSMUX_DIR\$sess.port"
$keyFile = "$PSMUX_DIR\$sess.key"
if (-not (Test-Path $portFile) -or -not (Test-Path $keyFile)) {
    Write-Host "FATAL: Port/key file not found at $PSMUX_DIR" -ForegroundColor Red
    & $psmux kill-session -t $sess 2>$null
    exit 1
}

# Verify AUTH works before running any tests
$authTest = Send-Tcp $sess "show-options -g mouse"
if ($authTest -eq "AUTH_FAILED" -or $authTest -match "^ERROR") {
    Write-Host "FATAL: Cannot authenticate to session $sess ($authTest)" -ForegroundColor Red
    & $psmux kill-session -t $sess 2>$null
    exit 1
}
Test-Result "auth-handshake" ($authTest -match 'mouse') "AUTH+show-options works: '$authTest'"

# ============================================================
# SECTION 1: Boolean options in apply_set_option + get_option_value
# Only options that exist in BOTH the setter AND getter in server/options.rs
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
    Send-Tcp $sess "set-option -g $opt on" | Out-Null
    $show = Send-Tcp $sess "show-options -g $opt"
    Test-Result "bool-on-$opt" ($show -match '\bon\b') "show='$show'"

    Send-Tcp $sess "set-option -g $opt off" | Out-Null
    $show = Send-Tcp $sess "show-options -g $opt"
    Test-Result "bool-off-$opt" ($show -match '\boff\b') "show='$show'"
}

# ============================================================
# SECTION 2: Numeric options set + VERIFY exact value
# Only options in both apply_set_option AND get_option_value
# ============================================================

$nums = @(
    @{n='escape-time'; v='100'; d='500'},
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
    Send-Tcp $sess "set-option -g $($opt.n) $($opt.v)" | Out-Null
    $show = Send-Tcp $sess "show-options -g $($opt.n)"
    Test-Result "num-$($opt.n)" ($show -match "\b$($opt.v)\b") "Expected $($opt.v), show='$show'"
    Send-Tcp $sess "set-option -g $($opt.n) $($opt.d)" | Out-Null
}

# ============================================================
# SECTION 3: String/style options set + VERIFY pattern
# ============================================================

$strings = @(
    @{n='status-left'; v='[TCP]'; p='TCP'},
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
    Send-Tcp $sess "set-option -g $($opt.n) $($opt.v)" | Out-Null
    $show = Send-Tcp $sess "show-options -g $($opt.n)"
    Test-Result "str-$($opt.n)" ($show -match $opt.p) "Expected '$($opt.p)', show='$show'"
}

# ============================================================
# SECTION 4: Flag tests using SEPARATE flags (TCP server requires exact tokens)
# ============================================================

# -g (global) + verify
Send-Tcp $sess "set-option -g escape-time 42" | Out-Null
$show = Send-Tcp $sess "show-options -g escape-time"
Test-Result "flag-g" ($show -match '\b42\b') "show='$show'"

# -a (append) using SEPARATE -a flag
Send-Tcp $sess "set-option -g status-right AAA" | Out-Null
Send-Tcp $sess "set-option -a status-right BBB" | Out-Null
$show = Send-Tcp $sess "show-options -g status-right"
Test-Result "flag-a-append" ($show -match 'AAABBB') "show='$show'"

# Triple append using separate flags
Send-Tcp $sess "set-option -g status-left X" | Out-Null
Send-Tcp $sess "set-option -a status-left Y" | Out-Null
Send-Tcp $sess "set-option -a status-left Z" | Out-Null
$show = Send-Tcp $sess "show-options -g status-left"
Test-Result "flag-a-triple" ($show -match 'XYZ') "show='$show'"

# -u (unset) on @user option using separate -u flag
Send-Tcp $sess "set-option -g @tcp-u-test value" | Out-Null
$show1 = Send-Tcp $sess "show-options -g @tcp-u-test"
Test-Result "flag-u-set-first" ($show1 -match 'value') "pre-unset: show='$show1'"
Send-Tcp $sess "set-option -u @tcp-u-test" | Out-Null
$show2 = Send-Tcp $sess "show-options -g @tcp-u-test"
Test-Result "flag-u-user-gone" ($show2 -notmatch 'value') "post-unset: show='$show2'"

# -q (quiet) + verify no error on bogus option
$resp = Send-Tcp $sess "set-option -q nonexistent-xyz val"
Test-Result "flag-q-silent" ($resp -notmatch 'unknown option') "resp='$resp'"

# -w (window scope) + verify accepted
Send-Tcp $sess "set-option -w mouse on" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "flag-w-accepted" ($show -match '\bon\b') "show='$show'"

# -t target + verify applied
Send-Tcp $sess "set-option -t 0 -g escape-time 99" | Out-Null
$show = Send-Tcp $sess "show-options -g escape-time"
Test-Result "flag-t-target" ($show -match '99') "show='$show'"

# Restore
Send-Tcp $sess "set-option -g escape-time 500" | Out-Null

# ============================================================
# SECTION 5: User/@- options full lifecycle (all verified)
# ============================================================

# Create + verify
Send-Tcp $sess "set-option -g @theme mocha" | Out-Null
$show = Send-Tcp $sess "show-options -g @theme"
Test-Result "user-create" ($show -match 'mocha') "show='$show'"

# Append using separate -a flag + verify concatenation
Send-Tcp $sess "set-option -a @theme _extended" | Out-Null
$show = Send-Tcp $sess "show-options -g @theme"
Test-Result "user-append" ($show -match 'mocha_extended') "show='$show'"

# Overwrite + verify new value
Send-Tcp $sess "set-option -g @theme latte" | Out-Null
$show = Send-Tcp $sess "show-options -g @theme"
Test-Result "user-overwrite" ($show -match 'latte') "show='$show'"

# Unset using separate -u flag + verify gone
Send-Tcp $sess "set-option -u @theme" | Out-Null
$show = Send-Tcp $sess "show-options -g @theme"
Test-Result "user-unset" ($show -notmatch 'latte') "show='$show'"

# ============================================================
# SECTION 6: Hooks lifecycle via TCP + VERIFIED via show-hooks
# ============================================================

Send-Tcp $sess "set-hook -g after-new-window 'run-shell echo hook1'" | Out-Null
$hooks = Send-Tcp $sess "show-hooks"
Test-Result "hook-set-verify" ($hooks -match 'after-new-window.*hook1') "hooks='$hooks'"

# Append hook + verify both present
Send-Tcp $sess "set-hook -a after-new-window 'run-shell echo hook2'" | Out-Null
$hooks = Send-Tcp $sess "show-hooks"
Test-Result "hook-append-verify" ($hooks -match 'hook1' -and $hooks -match 'hook2') "hooks='$hooks'"

# Unset hook + verify gone
Send-Tcp $sess "set-hook -u after-new-window" | Out-Null
$hooks = Send-Tcp $sess "show-hooks"
Test-Result "hook-unset-verify" ($hooks -notmatch 'after-new-window') "hooks='$hooks'"

# ============================================================
# SECTION 7: Environment via TCP + VERIFIED via show-environment
# ============================================================

Send-Tcp $sess "set-environment TCP_ENV_V1 alpha" | Out-Null
$env_out = Send-Tcp $sess "show-environment"
Test-Result "env-set-verify" ($env_out -match 'TCP_ENV_V1.*alpha') "env='$($env_out.Substring(0, [Math]::Min(200, $env_out.Length)))'"

Send-Tcp $sess "setenv TCP_ENV_V2 beta" | Out-Null
$env_out = Send-Tcp $sess "show-environment"
Test-Result "env-alias-verify" ($env_out -match 'TCP_ENV_V2.*beta') "env contains TCP_ENV_V2"

Send-Tcp $sess "set-environment -g TCP_ENV_G gamma" | Out-Null
# -g is the global environment, read back with -g like tmux (#775).
$env_out = Send-Tcp $sess "show-environment -g TCP_ENV_G"
Test-Result "env-global-verify" ($env_out -match 'TCP_ENV_G.*gamma') "env contains TCP_ENV_G"

# ============================================================
# SECTION 8: setw / set-window-option aliases + VERIFY
# ============================================================

Send-Tcp $sess "setw -g mode-keys vi" | Out-Null
$show = Send-Tcp $sess "show-options -g mode-keys"
Test-Result "setw-mode-keys" ($show -match 'vi') "show='$show'"

Send-Tcp $sess "set-window-option -g monitor-activity on" | Out-Null
$show = Send-Tcp $sess "show-options -g monitor-activity"
Test-Result "setwopt-monitor" ($show -match '\bon\b') "show='$show'"

# Restore
Send-Tcp $sess "setw -g mode-keys emacs" | Out-Null
Send-Tcp $sess "set-window-option -g monitor-activity off" | Out-Null

# ============================================================
# SECTION 9: show-options variants + VERIFY actual content
# ============================================================

$show = Send-Tcp $sess "show-options"
Test-Result "show-all-has-mouse" ($show -match 'mouse') "show-options contains 'mouse'"
Test-Result "show-all-has-status" ($show -match 'status') "show-options contains 'status'"
Test-Result "show-all-multiline" ($show.Split("`n").Count -gt 5) "show-options has >5 lines, got $($show.Split("`n").Count)"

$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "show-specific-format" ($show -match 'mouse\s+(on|off)') "show='$show'"

$show = Send-Tcp $sess "show-options -g escape-time"
Test-Result "show-g-numeric" ($show -match 'escape-time\s+\d+') "show='$show'"

# ============================================================
# SECTION 10: Command alias via TCP + VERIFY via show-options
# ============================================================

Send-Tcp $sess "set-option -g command-alias sp=split-window" | Out-Null
$show = Send-Tcp $sess "show-options -g command-alias"
Test-Result "cmd-alias-verify" ($show -match 'sp=split-window') "show='$show'"

Send-Tcp $sess "set-option -g command-alias nw=new-window" | Out-Null
$show = Send-Tcp $sess "show-options -g command-alias"
Test-Result "cmd-alias-second" ($show -match 'nw=new-window') "show='$show'"

# ============================================================
# SECTION 11: Status multiline + VERIFY via show-options
# ============================================================

Send-Tcp $sess "set-option -g status 2" | Out-Null
$show = Send-Tcp $sess "show-options -g status"
Test-Result "status-2" ($show -match '\b2\b') "show='$show'"

Send-Tcp $sess "set-option -g status 5" | Out-Null
$show = Send-Tcp $sess "show-options -g status"
Test-Result "status-5" ($show -match '\b5\b') "show='$show'"

# Restore
Send-Tcp $sess "set-option -g status on" | Out-Null

# ============================================================
# SECTION 12: Prefix via TCP + VERIFY via show-options
# ============================================================

Send-Tcp $sess "set-option -g prefix C-a" | Out-Null
$show = Send-Tcp $sess "show-options -g prefix"
Test-Result "prefix-c-a" ($show -match 'C-a') "show='$show'"

Send-Tcp $sess "set-option -g prefix2 C-s" | Out-Null
$show = Send-Tcp $sess "show-options -g prefix2"
Test-Result "prefix2-c-s" ($show -match 'C-s') "show='$show'"

Send-Tcp $sess "set-option -g prefix2 none" | Out-Null
$show = Send-Tcp $sess "show-options -g prefix2"
Test-Result "prefix2-none" ($show -match 'none' -or $show -notmatch 'C-s') "show='$show'"

# Restore
Send-Tcp $sess "set-option -g prefix C-b" | Out-Null

# ============================================================
# SECTION 13: user_options fallback storage + VERIFY
# ============================================================

$uo = @(
    'popup-style', 'popup-border-style', 'popup-border-lines',
    'window-style', 'window-active-style', 'wrap-search',
    'pane-border-format', 'pane-border-status',
    'clock-mode-colour', 'clock-mode-style',
    'lock-after-time', 'lock-command', 'status-keys'
)
foreach ($opt in $uo) {
    $uniq = "val$(Get-Random -Maximum 9999)"
    Send-Tcp $sess "set-option -g $opt $uniq" | Out-Null
    $show = Send-Tcp $sess "show-options -g $opt"
    Test-Result "uo-$opt" ($show -match $uniq) "Expected '$uniq', show='$show'"
}

# ============================================================
# SECTION 14: source-file via TCP + VERIFY loaded values
# ============================================================

$tempSrc = Join-Path $env:TEMP "psmux_tcp_source_$(Get-Random).conf"
@"
set -g escape-time 77
set -g base-index 5
"@ | Set-Content -Path $tempSrc -Encoding UTF8

Send-Tcp $sess "source-file $tempSrc" | Out-Null
Start-Sleep -Milliseconds 300
$show = Send-Tcp $sess "show-options -g escape-time"
Test-Result "source-et-77" ($show -match '\b77\b') "show='$show'"

$show = Send-Tcp $sess "show-options -g base-index"
Test-Result "source-bi-5" ($show -match '\b5\b') "show='$show'"

# Restore
Send-Tcp $sess "set-option -g escape-time 500" | Out-Null
Send-Tcp $sess "set-option -g base-index 0" | Out-Null
Remove-Item $tempSrc -Force -ErrorAction SilentlyContinue

# Nonexistent source-file: verify server still responds after (proves no crash)
Send-Tcp $sess "source-file /no/such/file.conf" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "source-missing-no-crash" ($show -match 'mouse') "Server still alive: show='$show'"

# ============================================================
# SECTION 15: tmux compat options + VERIFY server survives
# ============================================================

Send-Tcp $sess "set-option -g terminal-overrides ',xterm*:Tc'" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "compat-terminal-overrides" ($show -match 'mouse') "Server ok after terminal-overrides"

Send-Tcp $sess "set-option -g default-terminal xterm-256color" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "compat-default-terminal" ($show -match 'mouse') "Server ok after default-terminal"

Send-Tcp $sess "set-option -g update-environment 'FOO BAR'" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "compat-update-env" ($show -match 'mouse') "Server ok after update-environment"

# ============================================================
# SECTION 16: Boolean variant syntax verification
# ============================================================

# true/false
Send-Tcp $sess "set-option -g mouse true" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "boolvar-true" ($show -match '\bon\b') "show='$show'"

Send-Tcp $sess "set-option -g mouse false" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "boolvar-false" ($show -match '\boff\b') "show='$show'"

# 1/0
Send-Tcp $sess "set-option -g mouse 1" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "boolvar-1" ($show -match '\bon\b') "show='$show'"

Send-Tcp $sess "set-option -g mouse 0" | Out-Null
$show = Send-Tcp $sess "show-options -g mouse"
Test-Result "boolvar-0" ($show -match '\boff\b') "show='$show'"

# Restore
Send-Tcp $sess "set-option -g mouse on" | Out-Null

# ============================================================
# SECTION 17: Cross-channel verify (TCP set, CLI show)
# ============================================================

Send-Tcp $sess "set-option -g escape-time 321" | Out-Null
$cli = & $psmux show-options -t $sess -g escape-time 2>&1 | Out-String
Test-Result "cross-tcp-cli-et" ($cli -match '321') "CLI verify: '$cli'"

Send-Tcp $sess "set-option -g @cross-test tcpval" | Out-Null
$cli = & $psmux show-options -t $sess -g @cross-test 2>&1 | Out-String
Test-Result "cross-tcp-cli-user" ($cli -match 'tcpval') "CLI verify: '$cli'"

# Restore
Send-Tcp $sess "set-option -g escape-time 500" | Out-Null

# ============================================================
# Cleanup
# ============================================================

& $psmux kill-session -t $sess 2>$null

# ============================================================
# Summary
# ============================================================

Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  TCP Config Test Results" -ForegroundColor Cyan
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
