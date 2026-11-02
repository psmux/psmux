# Config FILE vs runtime option parity.
#
# `set -g alternate-screen off` in a config file used to warn
# "unknown option 'alternate-screen'" and leave `show -gv alternate-screen` at
# `on`, while the same `set -g` typed at runtime worked. The config parser has
# its own option match beside the runtime setter, and any catalog option that
# match forgot fell into the unknown option arm. message-limit and
# history-file-limit had the same shape (spurious warning).
#
# Proves end to end:
#   1. a config setting EVERY catalog option to a non default value starts
#      with no config warnings and every value reads back through show -gv,
#      identical to a server that received the same `set -g` at runtime
#   2. setw -g alternate-screen off in a config is honoured too
#   3. a pane in a config started server actually honours alternate-screen
#      off: text an app draws on the alternate screen survives its exit
#      (with the option on, the same text is gone), observed via capture-pane
#
# Isolation: unique -L namespaces, PSMUX_DATA_DIR scratch, cleanup only via
# `psmux -L <ns> kill-server`; the default namespace session list is compared
# before and after.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
Write-Host "psmux under test: $PSMUX"
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$scratch = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cfgparity_" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $scratch | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $scratch "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"

$defaultBefore = (& $PSMUX ls 2>&1 | Out-String).Trim()
$namespaces = @()
function New-Ns { $n = "alt_" + (Get-Random); $script:namespaces += $n; return $n }

function Start-WithConfig($ns, $content) {
    $conf = Join-Path $scratch "$ns.conf"
    $content | Set-Content -Path $conf -Encoding ascii
    $out = & $PSMUX -L $ns -f $conf new-session -d -s t -x 120 -y 30 2>&1 | Out-String
    for ($i = 0; $i -lt 50; $i++) {
        & $PSMUX -L $ns has-session -t t 2>$null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Milliseconds 200
    }
    return $out
}
function Get-Opt($ns, $name) { ((& $PSMUX -L $ns show -gv $name 2>&1) | Out-String).TrimEnd("`r", "`n") }

# Every catalog option except the three whose setter changes machine or
# process wide state the sweep has no business touching from a test
# (priority, bold-is-bright, codepoint-widths). warm and warm-pool-size are
# left out too: this script runs with PSMUX_NO_WARM=1, and a server whose warm
# pool is off after config deliberately zeroes the pool target
# (server/mod.rs post config reconcile), so the config server would read 0
# there by design. The unit sweep in tests-rs/test_config_option_parity.rs
# covers both names.
$opts = [ordered]@{
    'escape-time'='123'; 'focus-events'='on'; 'history-limit'='4321'; 'alternate-screen'='off'
    'set-clipboard'='off'; 'default-shell'='cmd.exe'; 'default-terminal'='screen-256color'; 'copy-command'='clip.exe'
    'terminal-overrides'='xterm*:smcup@:rmcup@'; 'exit-empty'='off'
    'prefix'='C-a'; 'prefix2'='C-q'; 'base-index'='1'; 'pane-base-index'='1'; 'display-time'='1234'
    'display-panes-time'='2345'; 'repeat-time'='600'; 'mouse'='off'; 'scroll-enter-copy-mode'='off'
    'mouse-drag-enter-copy-mode'='on'; 'pwsh-mouse-selection'='on'; 'mouse-selection'='off'
    'mouse-selection-force'='on'; 'paste-detection'='off'; 'mode-keys'='vi'; 'copy-mode-line-numbers'='relative'
    'copy-mode-line-number-style'='fg=red'; 'copy-mode-current-line-number-style'='fg=blue'; 'status'='off'
    'status-position'='top'; 'status-interval'='7'; 'status-justify'='centre'; 'status-left'='L#S'; 'status-right'='RR'
    'status-left-length'='22'; 'status-right-length'='33'; 'status-style'='bg=blue'; 'status-left-style'='fg=red'
    'status-right-style'='fg=cyan'; 'message-style'='bg=red'; 'message-command-style'='bg=magenta'; 'mode-style'='bg=cyan'
    'bell-action'='none'; 'visual-bell'='on'; 'activity-action'='none'; 'silence-action'='none'; 'monitor-silence'='9'
    'destroy-unattached'='on'; 'renumber-windows'='on'; 'set-titles'='on'; 'set-titles-string'='X#S'; 'word-separators'=' ,'
    'allow-passthrough'='on'; 'allow-rename'='off'; 'allow-set-title'='on'; 'update-environment'='FOO BAR'
    'synchronize-panes'='on'; 'choose-tree-preview'='on'; 'prediction-dimming'='on'; 'allow-predictions'='on'
    'cursor-style'='block'; 'cursor-blink'='off'; 'claude-code-fix-tty'='off'
    'claude-code-force-interactive'='off'; 'automatic-rename'='off'; 'monitor-activity'='on'; 'remain-on-exit'='on'
    'aggressive-resize'='on'; 'main-pane-width'='50'; 'main-pane-height'='40'; 'window-size'='largest'
    'window-status-format'='W#I'; 'window-status-current-format'='C#I'; 'window-status-separator'='|'
    'window-status-style'='fg=red'; 'window-status-current-style'='fg=blue'; 'window-status-activity-style'='bold'
    'window-status-bell-style'='underscore'; 'window-status-last-style'='italics'; 'pane-border-indicators'='arrows'
    'pane-border-style'='fg=red'; 'pane-active-border-style'='fg=blue'; 'pane-border-lines'='double'
    'pane-border-hover-style'='fg=cyan'; 'message-limit'='77'; 'history-file-limit'='88'
}

Write-Host "`n=== Config file vs runtime option parity ===" -ForegroundColor Cyan

try {
    # ---- 1: whole catalog sweep ----
    Write-Host "`n[Test 1] every catalog option via -f config matches runtime set -g" -ForegroundColor Yellow
    $nsCfg = New-Ns; $nsRt = New-Ns
    $cfgText = ($opts.GetEnumerator() | ForEach-Object { "set -g $($_.Key) `"$($_.Value)`"" }) -join "`n"
    $warn = Start-WithConfig $nsCfg $cfgText
    if ($warn -match 'config warning') { Write-Fail "config start warned:`n$warn" } else { Write-Pass "config start printed no warnings" }
    $null = Start-WithConfig $nsRt ""
    $rtErr = @()
    foreach ($k in $opts.Keys) {
        $e = (& $PSMUX -L $nsRt set -g $k $opts[$k] 2>&1 | Out-String).Trim()
        if ($e) { $rtErr += "$k : $e" }
    }
    if ($rtErr.Count -eq 0) { Write-Pass "runtime set -g accepted every value" } else { Write-Fail "runtime set -g errors:`n$($rtErr -join "`n")" }
    $bad = @()
    foreach ($k in $opts.Keys) {
        $a = (Get-Opt $nsCfg $k) -replace "`r", ''; $b = (Get-Opt $nsRt $k) -replace "`r", ''; $want = $opts[$k]
        # An array option prints one element a line with -v, as tmux 3.8 does
        # (update-environment splits `FOO BAR` into two elements).
        if ($k -eq 'update-environment') { $want = $want -replace ' ', "`n" }
        if ($a -ne $want -or $b -ne $want) { $bad += ("{0}: want [{1}] config [{2}] runtime [{3}]" -f $k, $want, $a, $b) }
    }
    if ($bad.Count -eq 0) { Write-Pass "all $($opts.Count) options read back identically on both paths" }
    else { Write-Fail "$($bad.Count) option(s) diverge:`n  $($bad -join "`n  ")" }
    $as = Get-Opt $nsCfg 'alternate-screen'
    if ($as -eq 'off') { Write-Pass "show -gv alternate-screen = off after config file set" } else { Write-Fail "show -gv alternate-screen = [$as] after config file set" }

    # ---- 2: setw -g spelling ----
    Write-Host "`n[Test 2] setw -g alternate-screen off in a config" -ForegroundColor Yellow
    $nsW = New-Ns
    $warn = Start-WithConfig $nsW "setw -g alternate-screen off"
    if ($warn -match 'config warning') { Write-Fail "setw config warned:`n$warn" } else { Write-Pass "setw config printed no warnings" }
    $v = Get-Opt $nsW 'alternate-screen'
    if ($v -eq 'off') { Write-Pass "setw -g alternate-screen off honoured" } else { Write-Fail "setw -g alternate-screen gave [$v]" }

    # ---- 3: a pane actually honours it ----
    Write-Host "`n[Test 3] pane honours alternate-screen from the config file" -ForegroundColor Yellow
    function Test-AltPane($value) {
        $ns = New-Ns
        $null = Start-WithConfig $ns "set -g alternate-screen $value`nset -g default-shell pwsh"
        for ($i = 0; $i -lt 60; $i++) {
            $cap = & $PSMUX -L $ns capture-pane -p -t t 2>&1 | Out-String
            if ($cap -match 'PS [A-Za-z]:') { break }
            Start-Sleep -Milliseconds 250
        }
        # The marker is split in the typed command so only the drawn output can
        # ever contain it whole.
        $cmd = '[Console]::Out.Write([char]27+"[?1049h"+"ALT"+"SCREENMARK"); Start-Sleep -Milliseconds 300; [Console]::Out.Write([char]27+"[?1049l")'
        & $PSMUX -L $ns send-keys -t t -l $cmd 2>&1 | Out-Null
        & $PSMUX -L $ns send-keys -t t Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 2500
        $cap = & $PSMUX -L $ns capture-pane -p -S -200 -t t 2>&1 | Out-String
        return ($cap -match 'ALTSCREENMARK')
    }
    $onKept = Test-AltPane 'on'
    $offKept = Test-AltPane 'off'
    Write-Host "    marker kept after alt screen exit: on=$onKept off=$offKept"
    if (-not $onKept) { Write-Pass "alternate-screen on: alt screen text discarded on exit (control)" }
    else { Write-Fail "alternate-screen on: control kept the alt screen text, probe cannot tell the modes apart" }
    if ($offKept) { Write-Pass "alternate-screen off from config: text drawn by the app stays in the main screen" }
    else { Write-Fail "alternate-screen off from config: pane still used the alternate screen" }
}
finally {
    foreach ($n in $namespaces) { & $PSMUX -L $n kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    $defaultAfter = (& $PSMUX ls 2>&1 | Out-String).Trim()
    if ($defaultAfter -eq $defaultBefore) { Write-Pass "default namespace session list unchanged" }
    else { Write-Fail "default namespace changed:`nbefore: $defaultBefore`nafter: $defaultAfter" }
    Remove-Item -Recurse -Force $scratch -EA SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
exit $script:TestsFailed
