# Issue #619 item 1: the ACTIVE pane falls back from window-active-style to
# window-style, per attribute, the way tmux tty.c tty_default_colours does.
#
#   if (wp == wp->window->active && wp->cached_active_gc.fg != 8)
#           gc->fg = wp->cached_active_gc.fg;
#   else
#           gc->fg = wp->cached_gc.fg;          /* and the same for bg */
#
# Proven on the raw pseudoconsole stream, so the assertion is the actual bytes
# the client writes to the terminal rather than an internal render buffer.

$ErrorActionPreference = "Continue"
# NO_COLOR strips SGR from the shell inside the pane in some agent shells and
# would hide the very sequences under test.
$env:NO_COLOR = $null
$PSMUX = (Get-Command psmux -EA Stop).Source
$HOSTEXE = "$env:TEMP\conpty_style_host.exe"
$BIN = "$env:TEMP\conpty_out.bin"
$CTRL = "$env:TEMP\conpty_ctrl.txt"
$SESSION = "i619_window_style_fallback"
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

function Kill-Session {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$env:USERPROFILE\.psmux\$SESSION.*" -Force -EA SilentlyContinue
}
# The bytes are read back through an inbox pseudo console, and the one of
# Windows 10 (19045) re-encodes a 256 colour index as the RGB of that index
# (measured there: 48;5;52 came out as 48;2;95;0;0, 38;5;201 as
# 38;2;255;0;255). Map such an RGB back to its index there, so the checks
# below still read what psmux wrote. Only exact xterm cube and grey values
# map, so the dim check's 48;2;57;0;0 stays as it is.
function ConvertFrom-Win10Rgb([string]$s) {
    if ([Environment]::OSVersion.Version.Build -ge 22000) { return $s }
    $lv = @(0, 95, 135, 175, 215, 255)
    $map = @{}
    for ($i = 16; $i -le 231; $i++) { $n = $i - 16; $map["$($lv[[int][math]::Floor($n / 36)]);$($lv[[int][math]::Floor($n / 6) % 6]);$($lv[$n % 6])"] = $i }
    for ($i = 232; $i -le 255; $i++) { $g = 8 + 10 * ($i - 232); $map["$g;$g;$g"] = $i }
    return [regex]::Replace($s, '(38|48);2;(\d+);(\d+);(\d+)', { param($m) $k = "$($m.Groups[2].Value);$($m.Groups[3].Value);$($m.Groups[4].Value)"; if ($map.ContainsKey($k)) { "$($m.Groups[1].Value);5;$($map[$k])" } else { $m.Value } })
}
function Read-BinEscaped {
    if (-not (Test-Path $BIN)) { return "" }
    $fs = [System.IO.File]::Open($BIN, 'Open', 'Read', 'ReadWrite')
    $buf = New-Object byte[] $fs.Length
    [void]$fs.Read($buf, 0, $fs.Length)
    $fs.Close()
    $sb = New-Object System.Text.StringBuilder
    foreach ($b in $buf) {
        if ($b -eq 0x1b) { [void]$sb.Append("<ESC>") }
        elseif ($b -ge 32 -and $b -lt 127) { [void]$sb.Append([char]$b) }
        elseif ($b -eq 0x0a) { [void]$sb.Append("\n") } else { [void]$sb.Append(("<{0:X2}>" -f $b)) }
    }
    return (ConvertFrom-Win10Rgb $sb.ToString())
}

# Boots a fresh single pane session under a real pseudoconsole, applies the
# option setup, forces a repaint and returns the escaped byte stream.
function Get-Stream([scriptblock]$setup) {
    Kill-Session
    Remove-Item $BIN,$CTRL -Force -EA SilentlyContinue
    $p = Start-Process -FilePath $HOSTEXE `
        -ArgumentList "`"$PSMUX`" new-session -s $SESSION" -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 5
    & $setup
    Start-Sleep -Milliseconds 800
    & $PSMUX refresh-client -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $esc = Read-BinEscaped
    Set-Content -Path $CTRL -Value "QUIT`n" -NoNewline
    Start-Sleep -Milliseconds 400
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Kill-Session
    return $esc
}

Write-Host "`n=== #619 window-style fallback for the active pane (raw pseudoconsole proof) ===" -ForegroundColor Cyan

# 1. window-style alone must tint the active pane. This is the regression: the
#    live client used to pick window-active-style outright, so an unset
#    window-active-style meant the active pane got no style at all.
Write-Host "`n[window-style alone tints the active pane]" -ForegroundColor Yellow
$esc = Get-Stream {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-style "bg=colour52" 2>&1 | Out-Null
}
if ($esc -match "48;5;52") {
    Write-Pass "window-style bg=colour52 reaches the active pane (48;5;52 present)"
} else {
    Write-Fail "window-style bg=colour52 missing from the active pane stream (48;5;52 absent)"
}

# 2. Per attribute fallback: fg comes from window-active-style, bg from
#    window-style, because tty_default_colours branches on fg and bg apart.
Write-Host "`n[fg and bg fall back independently]" -ForegroundColor Yellow
$esc = Get-Stream {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-style "bg=colour21" 2>&1 | Out-Null
    & $PSMUX set-option -g window-active-style "fg=colour196" 2>&1 | Out-Null
}
$fg = $esc -match "38;5;196"
$bg = $esc -match "48;5;21"
if ($fg -and $bg) {
    Write-Pass "active pane paints window-active-style fg over window-style bg (38;5;196 + 48;5;21)"
} else {
    Write-Fail "per attribute fallback wrong (fg 38;5;196 present=$fg, bg 48;5;21 present=$bg)"
}

# 3. window-active-style still wins where it names the attribute.
#    The stream is cumulative from session start, so window-active-style goes
#    first: with the fallback in place, window-style alone would tint the
#    active pane during the repaint between the two commands and 48;5;21
#    would appear legitimately. Ordered this way it must never appear.
Write-Host "`n[window-active-style overrides window-style where it names a colour]" -ForegroundColor Yellow
$esc = Get-Stream {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-active-style "bg=colour52" 2>&1 | Out-Null
    & $PSMUX set-option -g window-style "bg=colour21" 2>&1 | Out-Null
}
if (($esc -match "48;5;52") -and ($esc -notmatch "48;5;21")) {
    Write-Pass "active pane uses window-active-style bg (48;5;52 present, 48;5;21 absent)"
} else {
    Write-Fail "override wrong (48;5;52 present=$($esc -match '48;5;52'), 48;5;21 present=$($esc -match '48;5;21'))"
}

# 4. bg=default names no colour (tmux colour 8), so it must fall back.
Write-Host "`n[window-active-style bg=default inherits window-style]" -ForegroundColor Yellow
$esc = Get-Stream {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-style "bg=colour52" 2>&1 | Out-Null
    & $PSMUX set-option -g window-active-style "bg=default" 2>&1 | Out-Null
}
if ($esc -match "48;5;52") {
    Write-Pass "bg=default falls back to window-style (48;5;52 present)"
} else {
    Write-Fail "bg=default blocked the window-style fallback (48;5;52 absent)"
}

# 5. dim=N is read out of the window styles and applied like tmux colour_dim.
#    colour52 is 0x5f0000, so 40 percent off leaves 0x5f * 60 / 100 = 57.
Write-Host "`n[dim=N scales the window style colours]" -ForegroundColor Yellow
$esc = Get-Stream {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-active-style "bg=colour52,dim=40" 2>&1 | Out-Null
}
if (($esc -match "48;2;57;0;0") -and ($esc -notmatch "48;5;52")) {
    Write-Pass "dim=40 dims colour52 to RGB 57,0,0 (48;2;57;0;0 present, 48;5;52 absent)"
} else {
    Write-Fail "dim=40 not applied (48;2;57;0;0 present=$($esc -match '48;2;57;0;0'), undimmed 48;5;52 present=$($esc -match '48;5;52'))"
}

# 6. dim does NOT fall back between the two styles: tmux tty_default_colours
#    takes cached_active_dim for the active pane outright, so a dim that lives
#    only on window-style must not touch the active pane.
Write-Host "`n[dim does not fall back to window-style for the active pane]" -ForegroundColor Yellow
$esc = Get-Stream {
    & $PSMUX set-option -g status off 2>&1 | Out-Null
    & $PSMUX set-option -g window-style "bg=colour52,dim=40" 2>&1 | Out-Null
}
if (($esc -match "48;5;52") -and ($esc -notmatch "48;2;57;0;0")) {
    Write-Pass "active pane inherits the colour but not the dim (48;5;52 present, dimmed form absent)"
} else {
    Write-Fail "dim leaked across the styles (48;5;52 present=$($esc -match '48;5;52'), dimmed present=$($esc -match '48;2;57;0;0'))"
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
