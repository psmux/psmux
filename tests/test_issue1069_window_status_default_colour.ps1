# PR #1069: a window tab whose style says fg=default or bg=default keeps the
# status bar's colour, as tmux 3.8 does.
#
# tmux puts window-status-style and window-status-current-style into the
# window list format as #[...] (options-table.c:129, 153-155). format_draw
# parses each #[...] against the status line's cell (format-draw.c:828), and
# style_parse resolves `fg=default` and `bg=default` to that base cell's
# colour (style.c:221-233). So with the solarized theme's
#   status-style bg=colour235  and  window-status-style fg=colour244,bg=default
# the tabs keep the colour235 bar. psmux painted them with the terminal's
# own background, dark boxes on the bar.
#
# status-style itself starts from the default cell (status.c:248, style_apply
# then style_add, style.c:441-473), so `status-style bg=default` still means
# the terminal background: that is checked too.
#
# A real attached client draws the status line and tests\conread.cs -a reads
# its console cells. The legacy console attribute word carries the colour:
# foreground in bits 0..3, background in bits 4..7 (green bg 0x20, red fg 0x04,
# yellow fg 0x06). Named colours are used so the console maps them exactly.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server` plus the client PID. PSMUX_TEST_BIN or
# PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
$env:PSMUX_RUN_NOPAUSE = "1"

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_1069_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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
# console row holding $marker as @{ Text; Attrs }, one attribute word per column.
function Status-Row([string[]]$config, [string]$marker, [string[]]$after = @()) {
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Value $config
    $script:NS = "r16pr1069_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 80 -y 12 | Out-Null
    foreach ($c in $after) { P @($c -split ' ') | Out-Null }
    Start-Sleep -Milliseconds 1500
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $script:NS, "attach", "-t", "s" -PassThru
    Start-Sleep -Seconds 4
    $lines = [string[]](& $READ $client.Id -a)
    Stop-Process -Id $client.Id -Force -EA SilentlyContinue
    Start-Sleep -Milliseconds 600
    P kill-server | Out-Null
    $script:NS = ""
    Start-Sleep -Milliseconds 400
    $line = $lines | Where-Object { $_ -match [regex]::Escape($marker) } | Select-Object -Last 1
    if (-not $line) { return $null }
    $m = [regex]::Match($line, '^\[attr ([^\]]+)\] (.*)$')
    $attrs = @()
    foreach ($part in $m.Groups[1].Value.Split(',')) { $v, $n = $part.Split('x'); $attrs += @([int]$v) * [int]$n }
    return @{ Text = $m.Groups[2].Value; Attrs = $attrs }
}

# The colour bits (fg 0..3, bg 4..7) of each character of $text, as hex words.
function Colours($row, [string]$text) {
    $at = $row.Text.IndexOf($text)
    if ($at -lt 0) { return "no '$text'" }
    return (@($row.Attrs[$at..($at + $text.Length - 1)] | ForEach-Object { '{0:x2}' -f ($_ -band 0xff) }) -join ' ')
}

$GREEN_ON_BG = 0x20
$common = @(
    "set -g status-left 'QQQ '"
    "set -g status-right ''"
    "set -g window-status-format 'VVV'"
    "set -g window-status-current-format 'WWW'"
)

Write-Host "`n=== PR #1069: window tabs keep the status bar colour for a default fg or bg ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    if (-not (Test-Path $READ)) { Write-Fail "conread did not compile"; throw "no conread" }

    Write-Host "`n[1] window-status-style fg=red,bg=default on a green bar (other window)" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style fg=black,bg=green"
        "set -g window-status-style fg=red,bg=default"
    ) + $common) 'QQQ' @('new-window -d')
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        Check "colours of QQQ (status-left)" (Colours $row 'QQQ') '20 20 20'
        Check "colours of VVV (bg green from the bar, fg red)" (Colours $row 'VVV') '24 24 24'
    }

    Write-Host "`n[2] window-status-current-style fg=red,bg=default on a green bar (current window)" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style fg=black,bg=green"
        "set -g window-status-current-style fg=red,bg=default"
    ) + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else { Check "colours of WWW (bg green from the bar, fg red)" (Colours $row 'WWW') '24 24 24' }

    Write-Host "`n[3] fg=default takes the bar's foreground" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style fg=yellow,bg=green"
        "set -g window-status-current-style fg=default,bg=red"
    ) + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else { Check "colours of WWW (fg yellow from the bar, bg red)" (Colours $row 'WWW') '46 46 46' }

    Write-Host "`n[4] the solarized theme lines (colour235 bar, colour244 tabs, bg=default)" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style fg=colour136,bg=colour235"
        "set -g window-status-style fg=colour244,bg=default"
        "set -g window-status-current-style fg=colour166,bg=default"
    ) + $common) 'QQQ' @('new-window -d')
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        $at = $row.Text.IndexOf('QQQ')
        $barBg = ($row.Attrs[$at] -band 0xf0)
        $v = $row.Text.IndexOf('VVV'); $w = $row.Text.IndexOf('WWW')
        Check "bg of VVV matches the bar's bg" ('{0:x2}' -f ($row.Attrs[$v] -band 0xf0)) ('{0:x2}' -f $barBg)
        Check "bg of WWW matches the bar's bg" ('{0:x2}' -f ($row.Attrs[$w] -band 0xf0)) ('{0:x2}' -f $barBg)
    }

    Write-Host "`n[5] status-style bg=default still means the terminal background" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style fg=red,bg=default"
        "set -g window-status-current-style fg=yellow,bg=default"
    ) + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else {
        $at = $row.Text.IndexOf('QQQ')
        $bg = $row.Attrs[$at] -band 0xf0
        $last = $row.Attrs[-1] -band 0xf0
        Check "bg of QQQ is the terminal default (bg of an empty status cell)" ('{0:x2}' -f $bg) ('{0:x2}' -f $last)
        Check "bg of QQQ is not green" ([int]($bg -eq $GREEN_ON_BG)) '0'
        Check "colours of WWW (fg yellow, terminal bg)" (Colours $row 'WWW') (@(1..3 | ForEach-Object { '{0:x2}' -f ($bg -bor 0x06) }) -join ' ')
    }

    Write-Host "`n[6] a set colour in the window style still wins over the bar" -ForegroundColor Yellow
    $row = Status-Row (@(
        "set -g status-style fg=black,bg=green"
        "set -g window-status-current-style fg=yellow,bg=red"
    ) + $common) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else { Check "colours of WWW (fg yellow, bg red)" (Colours $row 'WWW') '46 46 46' }

    # The default status-format pushes the window style as the default before
    # it draws window-status-format (options-table.c:146 #[push-default]), so
    # an inline bg=default inside the window format means the TAB's colour
    # (blue here), not the bar's. Measured on tmux: ESC[31m ESC[44m WWW.
    Write-Host "`n[7] #[fg=red,bg=default] inside window-status-current-format" -ForegroundColor Yellow
    $row = Status-Row @(
        "set -g status-style fg=black,bg=green"
        "set -g status-left 'QQQ '"
        "set -g status-right ''"
        "set -g window-status-current-style fg=yellow,bg=blue"
        "set -g window-status-current-format '#[fg=red,bg=default]WWW'"
    ) 'QQQ'
    if ($null -eq $row) { Write-Fail "no status row with QQQ" }
    else { Check "colours of WWW (bg blue from the tab style, fg red)" (Colours $row 'WWW') '14 14 14' }
}
catch {
    if ("$_" -ne 'no conread') { Write-Fail "exception: $_" }
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
