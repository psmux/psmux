# Issue #973: status-style, window-status-style and window-status-current-style
# keep every attribute, and the `overline` and `nooverline` style names draw
# (tmux attributes.c:81, style.c:245-254 at tag 3.8).
#
# tmux takes the whole of status-style for the status line's default cell
# (status.c:248, style_apply at style.c:468-473) and puts the window styles
# into the window list format as `#[...]` (options-table.c:129, 153-155), so
# the window text keeps what status-style set and adds its own. The client
# kept the colours and bold alone: `status-style reverse`, `underscore` or
# `overline`, and `window-status-current-style underscore`, drew nothing.
#
# tmux 3.8 (WSL), status-left 'QQQ ', window-status-current-format 'WWW':
#   status-style bg=green,reverse          -> ESC[42m ESC[7m QQQ ESC[4m WWW (the
#                                             3.8 default current style is underscore)
#   status-style bg=green,underscore       -> ESC[42m ESC[4m QQQ WWW
#   window-status-current-style reverse    -> ESC[42m QQQ ESC[7m WWW
# tmux writes the overline (ESC[53m) only once the terminal has the `overline`
# feature (`set -as terminal-features ',xterm*:overline'`); psmux writes it to
# every terminal, as it does the extended underline styles.
#
# A real attached client draws the status line; its console cells are read
# with tests\conread.cs -a: COMMON_LVB_REVERSE_VIDEO 0x4000,
# COMMON_LVB_GRID_HORIZONTAL (the overline) 0x0400, and bold as
# FOREGROUND_INTENSITY 0x0008 on the black foreground. The console host reads
# SGR 4 back with no COMMON_LVB_UNDERSCORE, also for a `#[underscore]` that
# master already draws, so the underscore is left to the unit tests
# (tests-rs/test_style_attrs.rs) and reverse stands in for it here.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server` plus the client PID. PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

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
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$REVERSE = 0x4000
$OVERLINE = 0x0400
$INTENSITY = 0x0008

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ssa_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

$toolDir = Join-Path $TempHome "tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$READ = Join-Path $toolDir "conread.exe"
& $csc /nologo /optimize "/out:$READ" (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null

# Starts a server with $config, attaches a real client, and returns the
# console row holding $marker as @{ Text; Attrs } (or $null when the row is
# not found), one attribute word per column.
function Status-Row([string[]]$config, [string]$marker, [string[]]$after = @()) {
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Value $config
    $script:NS = "ssa_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 80 -y 12 | Out-Null
    foreach ($c in $after) { P @($c -split ' ') | Out-Null }
    Start-Sleep -Milliseconds 1500
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $script:NS, "attach", "-t", "s" -PassThru
    Start-Sleep -Seconds 4
    $lines = [string[]](& $READ $client.Id -a)
    Stop-Process -Id $client.Id -Force -EA SilentlyContinue
    Start-Sleep -Milliseconds 600
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 400
    $line = $lines | Where-Object { $_ -match [regex]::Escape($marker) } | Select-Object -Last 1
    if (-not $line) { return $null }
    $m = [regex]::Match($line, '^\[attr ([^\]]+)\] (.*)$')
    $attrs = @()
    foreach ($part in $m.Groups[1].Value.Split(',')) { $v, $n = $part.Split('x'); $attrs += @([int]$v) * [int]$n }
    return @{ Text = $m.Groups[2].Value; Attrs = $attrs }
}

# '1' or '0' per character of $text in $row, for the attribute bit $mask.
function Bits($row, [string]$text, [int]$mask) {
    $at = $row.Text.IndexOf($text)
    if ($at -lt 0) { return "no '$text'" }
    return (@($row.Attrs[$at..($at + $text.Length - 1)] | ForEach-Object { [int](($_ -band $mask) -ne 0) }) -join '')
}

# The status-left and the current window's text that the window cases read.
$common = @("set -g status-left 'QQQ '", "set -g status-right ''", "set -g window-status-current-format 'WWW'")

Write-Host "`n=== status-style and window styles keep every attribute ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    if (-not (Test-Path $READ)) { Write-Fail "conread did not compile"; throw "no conread" }

    Write-Host "`n[1] #[overline] and #[nooverline] in status-left" -ForegroundColor Yellow
    $row = Status-Row @(
        "set -g status-style bg=green"
        "set -g status-left '#[nooverline]L#[overline]O#[nooverline]P#[default] '"
        "set -g status-right ''"
    ) 'LOP'
    if ($null -eq $row) { Write-Fail "no status row with LOP" }
    else { Check "overline of L, O, P" (Bits $row 'LOP' $OVERLINE) '010' }

    Write-Host "`n[2] status-style overline, and nooverline after it" -ForegroundColor Yellow
    $row = Status-Row @(
        "set -g status-style 'bg=green,overline'"
        "set -g status-left 'AB#[nooverline]C '"
        "set -g status-right ''"
    ) 'ABC'
    if ($null -eq $row) { Write-Fail "no status row with ABC" }
    else { Check "overline of A, B, C" (Bits $row 'ABC' $OVERLINE) '110' }

    Write-Host "`n[3] status-style reverse reaches status-left, the window list and the rest of the line" -ForegroundColor Yellow
    $row = Status-Row (@("set -g status-style 'bg=green,reverse'") + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        Check "reverse of QQQ" (Bits $row 'QQQ' $REVERSE) '111'
        Check "reverse of WWW" (Bits $row 'WWW' $REVERSE) '111'
        Check "reverse of the last column" ([int](($row.Attrs[-1] -band $REVERSE) -ne 0)) '1'
    }

    Write-Host "`n[4] status-style overline, with the current window's reverse added on top" -ForegroundColor Yellow
    $row = Status-Row (@("set -g status-style 'bg=green,overline'", "set -g window-status-current-style reverse") + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        Check "overline of QQQ" (Bits $row 'QQQ' $OVERLINE) '111'
        Check "overline of WWW" (Bits $row 'WWW' $OVERLINE) '111'
        Check "reverse of QQQ" (Bits $row 'QQQ' $REVERSE) '000'
        Check "reverse of WWW" (Bits $row 'WWW' $REVERSE) '111'
    }

    Write-Host "`n[5] window-status-current-style reverse marks the current window alone" -ForegroundColor Yellow
    $row = Status-Row (@("set -g status-style bg=green", "set -g window-status-current-style reverse") + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        Check "reverse of QQQ" (Bits $row 'QQQ' $REVERSE) '000'
        Check "reverse of WWW" (Bits $row 'WWW' $REVERSE) '111'
    }

    Write-Host "`n[6] window-status-style reverse marks the other window" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style bg=green"
        "set -g window-status-style reverse"
        "set -g window-status-format 'VVV'"
    ) + $common) 'QQQ' @('new-window -d')
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        Check "reverse of VVV (not current)" (Bits $row 'VVV' $REVERSE) '111'
        Check "reverse of WWW (current)" (Bits $row 'WWW' $REVERSE) '000'
    }

    Write-Host "`n[7] status-style bold still draws, and a plain status line has none of them" -ForegroundColor Yellow
    $row = Status-Row (@("set -g status-style 'bg=green,bold'") + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else { Check "bold (intensity) of QQQ" (Bits $row 'QQQ' $INTENSITY) '111' }
    $row = Status-Row (@("set -g status-style bg=green") + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        $all = $REVERSE -bor $OVERLINE -bor $INTENSITY
        Check "attributes of QQQ" (Bits $row 'QQQ' $all) '000'
        Check "attributes of WWW" (Bits $row 'WWW' $all) '000'
    }

    # tmux style.c:243: `none` sets the attributes to 0, and the window style
    # is drawn as #[...] over status-style, so it clears status-style's too.
    Write-Host "`n[8] window-status-current-style none clears status-style's attributes on the current window" -ForegroundColor Yellow
    $row = Status-Row (@("set -g status-style 'bg=green,reverse,overline'", "set -g window-status-current-style 'none,fg=red'") + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        Check "reverse of QQQ" (Bits $row 'QQQ' $REVERSE) '111'
        Check "overline of QQQ" (Bits $row 'QQQ' $OVERLINE) '111'
        Check "reverse of WWW" (Bits $row 'WWW' $REVERSE) '000'
        Check "overline of WWW" (Bits $row 'WWW' $OVERLINE) '000'
    }
}
catch {
    if ("$_" -ne 'no conread') { Write-Fail "exception: $_" }
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
