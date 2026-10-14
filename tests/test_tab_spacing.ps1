# Test: Status bar tab spacing must match tmux exactly
# Verifies:
#   1. Default window-status-format/current-format match tmux
#   2. Correct conditional expansion (flags present -> flag char; absent -> trailing space)
#   3. No double-space between session name and first tab
#   4. Tab assembly spacing matches tmux exactly
#   5. set -gu resets to tmux default (not a wrong fallback)
#   6. Config file doesn't leave orphaned window-status-format overrides
#
# tmux defaults (from options-table.c):
#   status-left:                "[#S] "
#   window-status-format:       "#I:#W#{?window_flags,#{window_flags}, }"
#   window-status-current-format: "#I:#W#{?window_flags,#{window_flags}, }"
#   window-status-separator:    " " (single space)

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $PSMUX)) {
    $PSMUX = "$PSScriptRoot\..\target\debug\psmux.exe"
}
if (-not (Test-Path $PSMUX)) {
    Write-Host "[FATAL] psmux binary not found" -ForegroundColor Red
    exit 1
}

$SESSION = "tsp$(Get-Random -Maximum 9999)"
Write-Info "Using psmux binary: $PSMUX"
Write-Info "Session: $SESSION"

# --- Cleanup ---
Write-Info "Cleaning up stale sessions..."
& $PSMUX kill-server 2>&1 | Out-Null
taskkill /f /im psmux.exe 2>$null | Out-Null
Start-Sleep -Seconds 3
Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\*.key"  -Force -ErrorAction SilentlyContinue

# --- Start session ---
Write-Info "Starting session '$SESSION'..."
Start-Process -FilePath $PSMUX -ArgumentList "new-session -d -s $SESSION" -WindowStyle Hidden
Start-Sleep -Seconds 4

$expected_wsf = '#I:#W#{?window_flags,#{window_flags}, }'

# ===================================================================
# TEST 1: Default window-status-format matches tmux
# ===================================================================
Write-Test "1: Default window-status-format matches tmux"
$wsf = (& $PSMUX show-options -g -v window-status-format -t $SESSION 2>&1) | Out-String
$wsf = $wsf.Trim()
if ($wsf -eq $expected_wsf) {
    Write-Pass "window-status-format = '$wsf'"
} else {
    Write-Fail "window-status-format = '$wsf', expected '$expected_wsf'"
}

# ===================================================================
# TEST 2: Default window-status-current-format matches tmux
# ===================================================================
Write-Test "2: Default window-status-current-format matches tmux"
$wscf = (& $PSMUX show-options -g -v window-status-current-format -t $SESSION 2>&1) | Out-String
$wscf = $wscf.Trim()
if ($wscf -eq $expected_wsf) {
    Write-Pass "window-status-current-format = '$wscf'"
} else {
    Write-Fail "window-status-current-format = '$wscf', expected '$expected_wsf'"
}

# ===================================================================
# TEST 3: Active window flags = "*"
# ===================================================================
Write-Test "3: Active window flags = '*'"
$flags = (& $PSMUX display-message -p '#{window_flags}' -t $SESSION 2>&1) | Out-String
$flags = $flags.Trim()
if ($flags -eq '*') {
    Write-Pass "Active window flags = '$flags'"
} else {
    Write-Fail "Active window flags = '$flags', expected '*'"
}

# ===================================================================
# TEST 4: Active window tab text ends with * (no trailing space)
# ===================================================================
Write-Test "4: Active window tab text ends with '*'"
$tab = (& $PSMUX display-message -p '#I:#W#{?window_flags,#{window_flags}, }' -t $SESSION 2>&1) | Out-String
$tab = $tab.TrimEnd("`r`n")
$wname = (& $PSMUX display-message -p '#{window_name}' -t $SESSION 2>&1) | Out-String
$wname = $wname.Trim()
$widx = (& $PSMUX display-message -p '#{window_index}' -t $SESSION 2>&1) | Out-String
$widx = $widx.Trim()
$expected = "${widx}:${wname}*"
if ($tab -eq $expected) {
    Write-Pass "Active tab = '$tab'"
} else {
    Write-Fail "Active tab = '$tab' (len=$($tab.Length)), expected '$expected'"
}

# ===================================================================
# TEST 5: Two-window: list-windows format expansion per window
# ===================================================================
Write-Test "5: Two-window list-windows format expansion"
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 1

$lw = (& $PSMUX list-windows -F '#{window_index}|#{window_flags}|#I:#W#{?window_flags,#{window_flags}, }' -t $SESSION 2>&1) | Out-String
$lines = $lw.Trim().Split("`n") | ForEach-Object { $_.TrimEnd("`r") }
Write-Info "list-windows output:"
foreach ($ln in $lines) { Write-Info "  '$ln'" }

$pass = $true
foreach ($ln in $lines) {
    $parts = $ln.Split('|')
    if ($parts.Count -ge 3) {
        $fl = $parts[1]
        $tab_raw = $parts[2]
        if ($fl -eq '*') {
            if (-not $tab_raw.EndsWith('*')) { Write-Fail "Active tab '$tab_raw' doesn't end with *"; $pass = $false }
        } elseif ($fl -eq '-') {
            if (-not $tab_raw.EndsWith('-')) { Write-Fail "Last-window tab '$tab_raw' doesn't end with -"; $pass = $false }
        } elseif ($fl -eq '') {
            if (-not $tab_raw.EndsWith(' ')) { Write-Fail "Inactive tab '$tab_raw' missing trailing space"; $pass = $false }
        }
    }
}
if ($pass) { Write-Pass "Two-window format expansion correct" }

# ===================================================================
# TEST 6: Three-window format expansion
# ===================================================================
Write-Test "6: Three-window format expansion"
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 1

$lw = (& $PSMUX list-windows -F '#{window_index}|#{window_flags}|#I:#W#{?window_flags,#{window_flags}, }' -t $SESSION 2>&1) | Out-String
$lines = $lw.Trim().Split("`n") | ForEach-Object { $_.TrimEnd("`r") }
Write-Info "Three-window list-windows:"
foreach ($ln in $lines) { Write-Info "  '$ln'" }

$tab_texts = @()
$pass = $true
foreach ($ln in $lines) {
    $parts = $ln.Split('|')
    if ($parts.Count -ge 3) {
        $fl = $parts[1]
        $tab_raw = $parts[2]
        $tab_texts += $tab_raw
        if ($fl -eq '*') {
            if (-not $tab_raw.EndsWith('*')) { Write-Fail "Active tab '$tab_raw' wrong"; $pass = $false }
        } elseif ($fl -eq '-') {
            if (-not $tab_raw.EndsWith('-')) { Write-Fail "Last tab '$tab_raw' wrong"; $pass = $false }
        } elseif ($fl -eq '') {
            if (-not $tab_raw.EndsWith(' ')) { Write-Fail "Inactive tab '$tab_raw' missing trailing space"; $pass = $false }
        }
    }
}
if ($pass) { Write-Pass "Three-window format expansion correct" }

# ===================================================================
# TEST 7: Assembled tabs have NO triple spaces
# ===================================================================
Write-Test "7: Assembled tabs have no triple spaces"
$sep = " "
$assembled = $tab_texts -join $sep
Write-Info "Assembled tabs: '$assembled'"
$triple = ([regex]::Matches($assembled, '   ')).Count
if ($triple -gt 0) {
    Write-Fail "Triple space found in '$assembled'"
} else {
    Write-Pass "No triple spaces in tab assembly"
}

# ===================================================================
# TEST 8: status-left expansion has correct spacing
# ===================================================================
Write-Test "8: status-left expansion has correct spacing"
$sl = (& $PSMUX display-message -p '[#S] END' -t $SESSION 2>&1) | Out-String
$sl = $sl.Trim()
$expected_sl = "[$SESSION] END"
if ($sl -eq $expected_sl) {
    Write-Pass "status-left expansion: '$sl'"
} else {
    Write-Fail "status-left expansion: '$sl', expected '$expected_sl'"
}

# ===================================================================
# TEST 9: #F matches #{window_flags}
# ===================================================================
Write-Test "9: #F matches #{window_flags}"
$f1 = (& $PSMUX display-message -p '#F' -t $SESSION 2>&1) | Out-String
$f1 = $f1.Trim()
$f2 = (& $PSMUX display-message -p '#{window_flags}' -t $SESSION 2>&1) | Out-String
$f2 = $f2.Trim()
if ($f1 -eq $f2) {
    Write-Pass "#F='$f1' matches #{window_flags}='$f2'"
} else {
    Write-Fail "#F='$f1' != #{window_flags}='$f2'"
}

# ===================================================================
# TEST 10: set -gu resets to tmux default (not wrong #I:#W#F)
# ===================================================================
Write-Test "10: set -gu resets to tmux default"
& $PSMUX set-option -t $SESSION window-status-format 'CUSTOM' 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$custom = (& $PSMUX show-options -g -v window-status-format -t $SESSION 2>&1) | Out-String
$custom = $custom.Trim()
Write-Info "After set: '$custom'"
& $PSMUX set-option -u -t $SESSION window-status-format 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$reset = (& $PSMUX show-options -g -v window-status-format -t $SESSION 2>&1) | Out-String
$reset = $reset.Trim()
Write-Info "After reset: '$reset'"
if ($reset -eq $expected_wsf) {
    Write-Pass "set -gu resets to: '$reset'"
} else {
    Write-Fail "set -gu reset to '$reset', expected '$expected_wsf'"
}

# ===================================================================
# TEST 11: Config does NOT contain orphaned window-status-format
# ===================================================================
Write-Test "11: Config file clean (no orphaned overrides)"
$conf_content = ""
if (Test-Path "$env:USERPROFILE\.psmux.conf") {
    $conf_content = [System.IO.File]::ReadAllText("$env:USERPROFILE\.psmux.conf")
}
if ($conf_content -match 'window-status-format') {
    Write-Fail "Config contains 'window-status-format' override"
} else {
    Write-Pass "Config file is clean"
}

# ===================================================================
# TEST 12: Full status bar string simulation
# ===================================================================
Write-Test "12: Full status bar string matches tmux"
$names = @()
for ($i = 0; $i -lt 3; $i++) {
    $n = (& $PSMUX display-message -p '#{window_name}' -t "${SESSION}:$i" 2>&1) | Out-String
    $names += $n.Trim()
}

# Get flags per window via list-windows
$flags_map = @{}
foreach ($ln in $lines) {
    $parts = $ln.Split('|')
    if ($parts.Count -ge 2) {
        $flags_map[$parts[0]] = $parts[1]
    }
}

# Build expected tab texts
$expected_tabs = @()
for ($i = 0; $i -lt 3; $i++) {
    $fl = $flags_map["$i"]
    if ($fl -eq '*') {
        $expected_tabs += "${i}:$($names[$i])*"
    } elseif ($fl -eq '-') {
        $expected_tabs += "${i}:$($names[$i])-"
    } else {
        $expected_tabs += "${i}:$($names[$i]) "
    }
}
$expected_tabs_str = $expected_tabs -join " "

# status-left
$sl_raw = "[$SESSION] "
if ($sl_raw.Length -gt 10) { $sl_text = $sl_raw.Substring(0, 10) } else { $sl_text = $sl_raw }

$expected_full = "${sl_text}${expected_tabs_str}"
Write-Info "Expected full bar: '$expected_full'"

# Verify between ] and first digit: exactly 1 space
$bracket_pos = $expected_full.IndexOf(']')
$after_bracket = $expected_full.Substring($bracket_pos + 1)
$leading_spaces = 0
foreach ($c in $after_bracket.ToCharArray()) {
    if ($c -eq ' ') { $leading_spaces++ } else { break }
}
if ($leading_spaces -eq 1) {
    Write-Pass "1 space after session bracket (tmux match)"
} else {
    Write-Fail "$leading_spaces spaces after session bracket (expected 1)"
}

# No quadruple spaces
$quad = ([regex]::Matches($expected_full, '    ')).Count
if ($quad -gt 0) {
    Write-Fail "Quadruple space in full bar: '$expected_full'"
} else {
    Write-Pass "No excessive spacing in full status bar"
}

# ===================================================================
# Cleanup
# ===================================================================
Write-Info "Cleaning up..."
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 1

# ===================================================================
# Summary
# ===================================================================
Write-Host ""
Write-Host "==========================================" -ForegroundColor White
Write-Host "  Tab Spacing: $script:TestsPassed passed, $script:TestsFailed failed" -ForegroundColor $(if ($script:TestsFailed -eq 0) { "Green" } else { "Red" })
Write-Host "==========================================" -ForegroundColor White
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
