# End-to-end VT checks for live status and global pane-content style rendering.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$HOSTEXE = "$env:TEMP\conpty_style_host.exe"
$BIN = "$env:TEMP\conpty_out.bin"
$CTRL = "$env:TEMP\conpty_ctrl.txt"
$SESSION = "live_client_styles"
$NEEDLE = "38;5;201"
$REAPPLIED_NEEDLE = "38;5;202"
$COLOR = "fg=colour201,bg=colour21"
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }

# Compile the pseudoconsole host from the existing harness source.
if (-not (Test-Path $HOSTEXE)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize /out:$HOSTEXE tests\conpty_host_0xE.cs 2>&1 | Out-Null
}
if (-not (Test-Path $HOSTEXE)) { Write-Fail "could not compile conpty host"; exit 1 }

function Kill-Session { & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null; Start-Sleep -Milliseconds 400; Remove-Item "$env:USERPROFILE\.psmux\$SESSION.*" -Force -EA SilentlyContinue }
function Read-BinEscaped([long]$Start = 0) {
    if (-not (Test-Path $BIN)) { return "" }
    $fs = [System.IO.File]::Open($BIN, 'Open', 'Read', 'ReadWrite')
    $len = [Math]::Max(0, $fs.Length - $Start)
    [void]$fs.Seek($Start, [System.IO.SeekOrigin]::Begin)
    $buf = New-Object byte[] $len
    [void]$fs.Read($buf, 0, $len)
    $fs.Close()
    $sb = New-Object System.Text.StringBuilder
    foreach ($b in $buf) {
        if ($b -eq 0x1b) { [void]$sb.Append("<ESC>") }
        elseif ($b -ge 32 -and $b -lt 127) { [void]$sb.Append([char]$b) }
        elseif ($b -eq 0x0a) { [void]$sb.Append("\n") } else { [void]$sb.Append(("<{0:X2}>" -f $b)) }
    }
    return $sb.ToString()
}
function Start-Host { Remove-Item $BIN,$CTRL -Force -EA SilentlyContinue; Kill-Session
    $p = Start-Process -FilePath $HOSTEXE -ArgumentList "`"$PSMUX`" new-session -s $SESSION" -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 5; return $p }
function Start-Attach { Remove-Item $BIN,$CTRL -Force -EA SilentlyContinue
    $p = Start-Process -FilePath $HOSTEXE -ArgumentList "`"$PSMUX`" attach-session -t $SESSION" -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 5; return $p }
function Stop-Client($p) { Set-Content -Path $CTRL -Value "QUIT`n" -NoNewline; Start-Sleep -Milliseconds 400; try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
function Stop-Host($p) { Stop-Client $p; Kill-Session }

Write-Host "`n=== Live style rendering (raw pseudoconsole proof) ===" -ForegroundColor Cyan

# Runs a scenario and checks whether NEEDLE appears anywhere in the raw pseudoconsole stream.
function Test-Style($name, [scriptblock]$setup, [bool]$expectApplied = $true) {
    Write-Host "`n[$name]" -ForegroundColor Yellow
    $p = Start-Host
    & $setup
    Start-Sleep -Milliseconds 800
    & $PSMUX refresh-client -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $esc = Read-BinEscaped
    $applied = $esc -match $NEEDLE
    Stop-Host $p
    if ($applied -eq $expectApplied) {
        Write-Pass "$name -> colour $NEEDLE present=$applied (expected $expectApplied)"
    } else {
        Write-Fail "$name -> colour $NEEDLE present=$applied (expected $expectApplied)"
    }
}

# CONTROL: window-status-current-style must render (proves capture works).
Test-Style "CONTROL window-status-current-style" {
    & $PSMUX set-option -g window-status-current-style $COLOR 2>&1 | Out-Null
    & $PSMUX new-window -t $SESSION 2>&1 | Out-Null
}

# Global pane-content styles must render through the attached client.
Test-Style "window-active-style" {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-active-style $COLOR 2>&1 | Out-Null
}

Test-Style "window-active-style append" {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-active-style "bg=black" 2>&1 | Out-Null
    & $PSMUX set-option -ga window-active-style ",$COLOR" 2>&1 | Out-Null
}

Write-Host "`n[window-active-style unset clears the style]" -ForegroundColor Yellow
$p = Start-Host
& $PSMUX set-option -g status off 2>&1 | Out-Null
& $PSMUX send-keys -t $SESSION "Write-Host STYLEMARK" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
& $PSMUX set-option -g window-active-style $COLOR 2>&1 | Out-Null
Start-Sleep -Seconds 1
$beforeUnset = Read-BinEscaped
$offset = (Get-Item $BIN).Length
& $PSMUX set-option -gu window-active-style 2>&1 | Out-Null
Start-Sleep -Seconds 1
$existingClientAfterUnset = Read-BinEscaped -Start $offset
$null = Stop-Client $p
$p = Start-Attach
$afterUnset = Read-BinEscaped
$null = Stop-Client $p
& $PSMUX set-option -go window-active-style "fg=colour202" 2>&1 | Out-Null
$p = Start-Attach
$afterOnlyIfUnset = Read-BinEscaped
Stop-Host $p
if (
    $beforeUnset -match $NEEDLE -and
    $existingClientAfterUnset -match 'STYLEMARK' -and
    $existingClientAfterUnset -notmatch $NEEDLE -and
    $afterUnset -match '<ESC>\[' -and
    $afterUnset -notmatch $NEEDLE -and
    $afterOnlyIfUnset -match $REAPPLIED_NEEDLE -and
    $afterOnlyIfUnset -notmatch $NEEDLE
) {
    Write-Pass "window-active-style clears on unset and -o can apply a new value"
} else {
    Write-Fail "window-active-style reset evidence wrong (before=$($beforeUnset -match $NEEDLE) existingRepaint=$($existingClientAfterUnset -match 'STYLEMARK') existingCleared=$($existingClientAfterUnset -notmatch $NEEDLE) freshCleared=$($afterUnset -notmatch $NEEDLE) reapplied=$($afterOnlyIfUnset -match $REAPPLIED_NEEDLE))"
}

Test-Style "window-style on inactive pane" {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-style $COLOR 2>&1 | Out-Null
    & $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
}

# window-status-last-style
Test-Style "window-status-last-style" {
    & $PSMUX set-option -g window-status-last-style $COLOR 2>&1 | Out-Null
    & $PSMUX new-window -t $SESSION 2>&1 | Out-Null
    & $PSMUX select-window -t "${SESSION}:0" 2>&1 | Out-Null
    & $PSMUX select-window -t "${SESSION}:1" 2>&1 | Out-Null
}

# window-status-activity-style (background window emits output -> activity flag)
Test-Style "window-status-activity-style" {
    & $PSMUX set-option -g monitor-activity on 2>&1 | Out-Null
    & $PSMUX set-option -g window-status-activity-style $COLOR 2>&1 | Out-Null
    & $PSMUX new-window -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    & $PSMUX send-keys -t "${SESSION}:0" "echo actmark" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
}

# window-status-bell-style (background window emits BEL -> bell flag)
Test-Style "window-status-bell-style" {
    & $PSMUX set-option -g monitor-bell on 2>&1 | Out-Null
    & $PSMUX set-option -g window-status-bell-style $COLOR 2>&1 | Out-Null
    & $PSMUX new-window -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    & $PSMUX send-keys -t "${SESSION}:0" 'Write-Host "`a"' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
}

# status-left-style
Test-Style "status-left-style" {
    & $PSMUX set-option -g status-left "LMARK" 2>&1 | Out-Null
    & $PSMUX set-option -g status-left-style $COLOR 2>&1 | Out-Null
}

# status-right-style
Test-Style "status-right-style" {
    & $PSMUX set-option -g status-right "RMARK" 2>&1 | Out-Null
    & $PSMUX set-option -g status-right-style $COLOR 2>&1 | Out-Null
}

# ── Layer 2: Win32 TUI visual verification (visible window, CLI-driven) ──
Write-Host "`n[Layer 2] Win32 TUI visual verification" -ForegroundColor Yellow
$TUI = "live_client_styles_tui"
& $PSMUX kill-session -t $TUI 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$TUI -PassThru
Start-Sleep -Seconds 4
& $PSMUX set-option -g window-status-activity-style "fg=colour200" 2>&1 | Out-Null
& $PSMUX set-option -g window-status-bell-style "fg=colour200" 2>&1 | Out-Null
& $PSMUX set-option -g status-left-style "fg=colour200" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$a = (& $PSMUX show-options -g -v window-status-activity-style -t $TUI 2>&1).Trim()
$b = (& $PSMUX show-options -g -v window-status-bell-style -t $TUI 2>&1).Trim()
$l = (& $PSMUX show-options -g -v status-left-style -t $TUI 2>&1).Trim()
if ($a -eq "fg=colour200" -and $b -eq "fg=colour200" -and $l -eq "fg=colour200") { Write-Pass "TUI: options stored + session responsive after set" }
else { Write-Fail "TUI: options not reflected (a=$a b=$b l=$l)" }
$panes = (& $PSMUX display-message -t $TUI -p '#{window_panes}' 2>&1).Trim()
if ($panes -eq "1") { Write-Pass "TUI: session still functional (window_panes=1)" }
else { Write-Fail "TUI: session unresponsive (window_panes=$panes)" }
& $PSMUX kill-session -t $TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
