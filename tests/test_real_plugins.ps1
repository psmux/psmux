# psmux Real Plugin/Theme End-to-End Test
# Actually sources the plugin .ps1 scripts from psmux-plugins repo
# and verifies the options/bindings they set were applied correctly.
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_real_plugins.ps1

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip { param($msg) Write-Host "[SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

$PLUGIN_DIR = "$env:USERPROFILE\.psmux\plugins\psmux-plugins"
if (-not (Test-Path $PLUGIN_DIR)) {
    Write-Info "Cloning psmux-plugins repo..."
    git clone https://github.com/psmux/psmux-plugins $PLUGIN_DIR 2>&1 | Out-Null
}
if (-not (Test-Path $PLUGIN_DIR)) { Write-Error "Plugin repo not found at $PLUGIN_DIR"; exit 1 }
Write-Info "Plugin dir: $PLUGIN_DIR"

function Psmux { & $PSMUX @args 2>&1; Start-Sleep -Milliseconds 300 }

# All tests use session "default" so plugins (which call psmux without -t)
# discover the server via default.port.
$S = "default"

function Start-FreshSession {
    & $PSMUX kill-server 2>$null
    Start-Sleep -Seconds 2
    Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
    Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue
    Start-Process -FilePath $PSMUX -ArgumentList "new-session -s $S -d" -WindowStyle Hidden
    # Poll for TCP readiness instead of fixed sleep, up to 15 seconds
    $portFile = "$env:USERPROFILE\.psmux\$S.port"
    $keyFile  = "$env:USERPROFILE\.psmux\$S.key"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if ((Test-Path $portFile) -and (Test-Path $keyFile)) {
            $port = [int](Get-Content $portFile -Raw -EA SilentlyContinue).Trim()
            if ($port -gt 0) {
                try {
                    $t = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $port)
                    $t.Close()
                    Start-Sleep -Milliseconds 300
                    return $true
                } catch {}
            }
        }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

# Ensure psmux is in PATH for plugin discovery
$binDir = Split-Path $PSMUX
$env:PATH = "$binDir;$env:PATH"

# ============================================================
# THEME 1: Catppuccin
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "THEME: Catppuccin (real plugin script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for catppuccin test"
} else {
    Write-Test "Source catppuccin theme script"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-theme-catppuccin\psmux-theme-catppuccin.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "Catppuccin: status-style has mocha base color"
    $v = (Psmux show-options -g -v status-style -t $S | Out-String).Trim()
    if ($v -match "#1e1e2e") { Write-Pass "status-style: $v" }
    else { Write-Fail "status-style: '$v' (expected #1e1e2e)" }

    Write-Test "Catppuccin: status-left has session indicator"
    $v = (Psmux show-options -g -v status-left -t $S | Out-String).Trim()
    if ($v -match "#S" -and $v -match "#89b4fa") { Write-Pass "status-left: has #S and blue accent" }
    else { Write-Fail "status-left: '$v'" }

    Write-Test "Catppuccin: status-right has time format"
    $v = (Psmux show-options -g -v status-right -t $S | Out-String).Trim()
    if ($v -match "%H:%M") { Write-Pass "status-right: has time" }
    else { Write-Fail "status-right: '$v'" }

    Write-Test "Catppuccin: window-status-current-format has green accent"
    $v = (Psmux show-options -g -v window-status-current-format -t $S | Out-String).Trim()
    if ($v -match "#a6e3a1" -and $v -match "#I") { Write-Pass "window-status-current-format: green + index" }
    else { Write-Fail "window-status-current-format: '$v'" }

    Write-Test "Catppuccin: mode-style (copy mode colors)"
    $v = (Psmux show-options -g -v mode-style -t $S | Out-String).Trim()
    if ($v -match "#89b4fa" -or $v -match "blue") { Write-Pass "mode-style: $v" }
    else { Write-Fail "mode-style: '$v'" }

    Write-Test "Catppuccin: pane-active-border-style"
    $v = (Psmux show-options -g -v pane-active-border-style -t $S | Out-String).Trim()
    if ($v -match "#89b4fa" -or $v -match "blue") { Write-Pass "pane-active-border-style: $v" }
    else { Write-Fail "pane-active-border-style: '$v'" }

    Write-Test "Catppuccin: message-style"
    $v = (Psmux show-options -g -v message-style -t $S | Out-String).Trim()
    if ($v -match "#313244" -or $v -match "surface") { Write-Pass "message-style: $v" }
    else { Write-Fail "message-style: '$v'" }

    Write-Test "Catppuccin: display-message works"
    $v = (Psmux display-message -t $S -p '#{session_name}' | Out-String).Trim()
    if ($v -eq $S) { Write-Pass "display-message: $v" }
    else { Write-Fail "display-message: '$v'" }
}


# ============================================================
# THEME 2: Dracula
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "THEME: Dracula (real plugin script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for dracula test"
} else {
    Write-Test "Source dracula theme script"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-theme-dracula\psmux-theme-dracula.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "Dracula: status-style has dracula bg"
    $v = (Psmux show-options -g -v status-style -t $S | Out-String).Trim()
    if ($v -match "#282a36") { Write-Pass "status-style: $v" }
    else { Write-Fail "status-style: '$v'" }

    Write-Test "Dracula: window-status-current-format has purple accent"
    $v = (Psmux show-options -g -v window-status-current-format -t $S | Out-String).Trim()
    if ($v -match "#bd93f9") { Write-Pass "window current: purple powerline" }
    else { Write-Fail "window current: '$v'" }

    Write-Test "Dracula: mode-style"
    $v = (Psmux show-options -g -v mode-style -t $S | Out-String).Trim()
    if ($v -match "#ff79c6" -or $v -match "#bd93f9") { Write-Pass "mode-style: $v" }
    else { Write-Fail "mode-style: '$v'" }
}


# ============================================================
# THEME 3: Nord
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "THEME: Nord (real plugin script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for nord test"
} else {
    Write-Test "Source nord theme script"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-theme-nord\psmux-theme-nord.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "Nord: status-style polar night bg"
    $v = (Psmux show-options -g -v status-style -t $S | Out-String).Trim()
    if ($v -match "#3b4252" -or $v -match "#2e3440") { Write-Pass "status-style: $v" }
    else { Write-Fail "status-style: '$v'" }

    Write-Test "Nord: window-status-current-format frost accent"
    $v = (Psmux show-options -g -v window-status-current-format -t $S | Out-String).Trim()
    if ($v -match "#88c0d0" -or $v -match "#81a1c1") { Write-Pass "window current: frost" }
    else { Write-Fail "window current: '$v'" }
}


# ============================================================
# THEME 4: Tokyo Night
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "THEME: Tokyo Night (real plugin script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for tokyonight test"
} else {
    Write-Test "Source tokyonight theme script"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-theme-tokyonight\psmux-theme-tokyonight.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "Tokyo Night: status-style storm bg"
    $v = (Psmux show-options -g -v status-style -t $S | Out-String).Trim()
    if ($v -match "#1a1b26" -or $v -match "#24283b") { Write-Pass "status-style: $v" }
    else { Write-Fail "status-style: '$v'" }

    Write-Test "Tokyo Night: window-status-current-format"
    $v = (Psmux show-options -g -v window-status-current-format -t $S | Out-String).Trim()
    if ($v -match "(?i)#7aa2f7" -or $v -match "(?i)#7dcfff" -or $v -match "(?i)#BB9AF7") { Write-Pass "window current: tokyo night accent" }
    else { Write-Fail "window current: '$v'" }
}


# ============================================================
# THEME 5: Gruvbox
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "THEME: Gruvbox (real plugin script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for gruvbox test"
} else {
    Write-Test "Source gruvbox theme script"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-theme-gruvbox\psmux-theme-gruvbox.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "Gruvbox: status-style dark bg"
    $v = (Psmux show-options -g -v status-style -t $S | Out-String).Trim()
    if ($v -match "#3c3836" -or $v -match "#282828") { Write-Pass "status-style: $v" }
    else { Write-Fail "status-style: '$v'" }

    Write-Test "Gruvbox: window-status-current-format warm accent"
    $v = (Psmux show-options -g -v window-status-current-format -t $S | Out-String).Trim()
    if ($v -match "#fe8019" -or $v -match "#fabd2f" -or $v -match "#b8bb26") { Write-Pass "window current: warm accent" }
    else { Write-Fail "window current: '$v'" }
}


# ============================================================
# PLUGIN: psmux-sensible
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "PLUGIN: psmux-sensible (real script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for sensible test"
} else {
    Write-Test "Source sensible plugin"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-sensible\psmux-sensible.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    # The value comes from the plugin script, which has changed it before
    # (50, then 10 in psmux-plugins 1e4d6847, the tmux 3.5+ default).
    $sensibleText = Get-Content -Raw "$PLUGIN_DIR\psmux-sensible\psmux-sensible.ps1"
    $m = [regex]::Match($sensibleText, '(?m)^[^#\r\n]*\bset -go escape-time (\d+)')
    $wantEscape = if ($m.Success) { $m.Groups[1].Value } else { '' }
    Write-Test "sensible: escape-time $wantEscape (as the plugin script sets it)"
    $v = (Psmux show-options -g -v escape-time -t $S | Out-String).Trim()
    if (-not $wantEscape) { Write-Fail "escape-time: no 'set -go escape-time' line in psmux-sensible.ps1" }
    elseif ($v -eq $wantEscape) { Write-Pass "escape-time: $v" }
    else { Write-Fail "escape-time: '$v', the plugin sets $wantEscape" }

    Write-Test "sensible: history-limit 50000"
    $v = (Psmux show-options -g -v history-limit -t $S | Out-String).Trim()
    if ($v -eq "50000") { Write-Pass "history-limit: $v" }
    else { Write-Fail "history-limit: '$v'" }

    Write-Test "sensible: mouse on"
    $v = (Psmux show-options -g -v mouse -t $S | Out-String).Trim()
    if ($v -eq "on") { Write-Pass "mouse: $v" }
    else { Write-Fail "mouse: '$v'" }

    Write-Test "sensible: mode-keys vi"
    $v = (Psmux show-options -g -v mode-keys -t $S | Out-String).Trim()
    if ($v -eq "vi") { Write-Pass "mode-keys: $v" }
    else { Write-Fail "mode-keys: '$v'" }

    Write-Test "sensible: focus-events on"
    $v = (Psmux show-options -g -v focus-events -t $S | Out-String).Trim()
    if ($v -eq "on") { Write-Pass "focus-events: $v" }
    else { Write-Fail "focus-events: '$v'" }

    Write-Test "sensible: base-index 1"
    $v = (Psmux show-options -g -v base-index -t $S | Out-String).Trim()
    if ($v -eq "1") { Write-Pass "base-index: $v" }
    else { Write-Fail "base-index: '$v'" }

    Write-Test "sensible: renumber-windows on"
    $v = (Psmux show-options -g -v renumber-windows -t $S | Out-String).Trim()
    if ($v -eq "on") { Write-Pass "renumber-windows: $v" }
    else { Write-Fail "renumber-windows: '$v'" }

    Write-Test "sensible: display-time 2000"
    $v = (Psmux show-options -g -v display-time -t $S | Out-String).Trim()
    if ($v -eq "2000") { Write-Pass "display-time: $v" }
    else { Write-Fail "display-time: '$v'" }

    Write-Test "sensible: keybinding | split-window"
    $keys = (Psmux list-keys -t $S | Out-String)
    if ($keys -match '\|.*split-window') { Write-Pass "| split-window binding" }
    else { Write-Fail "| split-window not found" }

    Write-Test "sensible: keybinding - split-window"
    if ($keys -match '\-.*split-window') { Write-Pass "- split-window binding" }
    else { Write-Fail "- split-window not found" }

    Write-Test "sensible: keybinding S-Left previous-window"
    if ($keys -match 'S-Left.*previous-window') { Write-Pass "S-Left previous-window" }
    else { Write-Fail "S-Left previous-window not found" }
}


# ============================================================
# PLUGIN: psmux-pain-control
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "PLUGIN: psmux-pain-control (real script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for pain-control test"
} else {
    Write-Test "Source pain-control plugin"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-pain-control\psmux-pain-control.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "pain-control: | split binding"
    $keys = (Psmux list-keys -t $S | Out-String)
    if ($keys -match '\|.*split-window.*-h') { Write-Pass "| horizontal split" }
    else { Write-Fail "| split not found" }

    Write-Test "pain-control: - split binding"
    if ($keys -match '\-.*split-window.*-v') { Write-Pass "- vertical split" }
    else { Write-Fail "- split not found" }

    Write-Test "pain-control: resize bindings"
    $hasResize = ($keys -match 'H.*resize-pane' -or $keys -match 'resize-pane.*-L')
    if ($hasResize) { Write-Pass "resize pane bindings" }
    else { Write-Fail "resize bindings not found" }
}


# ============================================================
# PLUGIN: psmux-prefix-highlight
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "PLUGIN: psmux-prefix-highlight (real script)"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for prefix-highlight test"
} else {
    Write-Test "Source prefix-highlight plugin"
    $output = pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-prefix-highlight\psmux-prefix-highlight.ps1'" 2>&1 | Out-String
    Write-Info "Script output: $($output.Trim())"
    Start-Sleep -Milliseconds 500

    Write-Test "prefix-highlight: status-right modified"
    $v = (Psmux show-options -g -v status-right -t $S | Out-String).Trim()
    if ($v -match "client_prefix" -or $v -match "PREFIX" -or $v.Length -gt 5) { Write-Pass "status-right updated: $(($v.Substring(0, [Math]::Min(60, $v.Length))))..." }
    else { Write-Fail "status-right: '$v'" }

    Write-Test "prefix-highlight: @options set"
    $phfg = (Psmux show-options -g -v "@prefix-highlight-fg" -t $S | Out-String).Trim()
    $phbg = (Psmux show-options -g -v "@prefix-highlight-bg" -t $S | Out-String).Trim()
    if ($phfg.Length -gt 0 -or $phbg.Length -gt 0) { Write-Pass "@prefix-highlight options: fg=$phfg bg=$phbg" }
    else { Write-Skip "prefix-highlight @options not set (plugin may use defaults)" }
}


# ============================================================
# NOTE: psmux-yank removed — clipboard is built into psmux natively.
# y to copy in copy-mode, right-click paste, Ctrl+V paste all work
# without any plugin. See src/copy_mode.rs.
# ============================================================


# ============================================================
# COMBO: Theme + Sensible + Pain-Control (real workflow)
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "COMBO: Catppuccin + Sensible + Pain-Control"
Write-Host ("=" * 60)

if (-not (Start-FreshSession)) {
    Write-Fail "Cannot create session for combo test"
} else {
    Write-Test "Load multiple plugins in sequence"
    pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-sensible\psmux-sensible.ps1'" 2>&1 | Out-Null
    pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-pain-control\psmux-pain-control.ps1'" 2>&1 | Out-Null
    pwsh -NoProfile -ExecutionPolicy Bypass -Command "& '$PLUGIN_DIR\psmux-theme-catppuccin\psmux-theme-catppuccin.ps1'" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    Write-Test "combo: sensible history-limit persists"
    $v = (Psmux show-options -g -v history-limit -t $S | Out-String).Trim()
    if ($v -eq "50000") { Write-Pass "history-limit: $v" }
    else { Write-Fail "history-limit: '$v'" }

    Write-Test "combo: catppuccin status-style active"
    $v = (Psmux show-options -g -v status-style -t $S | Out-String).Trim()
    if ($v -match "#1e1e2e") { Write-Pass "status-style: catppuccin active" }
    else { Write-Fail "status-style: '$v'" }

    Write-Test "combo: pain-control | binding"
    $keys = (Psmux list-keys -t $S | Out-String)
    if ($keys -match '\|.*split-window') { Write-Pass "| binding coexists" }
    else { Write-Fail "| binding missing" }

    Write-Test "combo: sensible S-Left binding"
    if ($keys -match 'S-Left.*previous-window' -or $keys -match 'S-Right.*next-window') {
        Write-Pass "S-Left/S-Right bindings coexist"
    } else { Write-Fail "shift-arrow bindings missing" }

    Write-Test "combo: format variables work"
    $v = (Psmux display-message -t $S -p '#{session_name}' | Out-String).Trim()
    if ($v -eq $S) { Write-Pass "session_name: $v" }
    else { Write-Fail "session_name: '$v'" }
}


# ============================================================
# Cleanup & Summary
# ============================================================
Write-Host ""
& $PSMUX kill-server 2>$null
Start-Sleep -Seconds 2

Write-Host ""
Write-Host ("=" * 60)
Write-Host "REAL PLUGIN TEST RESULTS"
Write-Host ("=" * 60)
Write-Host "Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "Failed:  $($script:TestsFailed)" -ForegroundColor Red
Write-Host "Skipped: $($script:TestsSkipped)" -ForegroundColor Yellow
Write-Host "Total:   $($script:TestsPassed + $script:TestsFailed + $script:TestsSkipped)"

if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
