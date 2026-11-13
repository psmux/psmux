# PR #1046: clock mode owns the whole pane. tmux draws clock mode on its own
# screen and window_clock_draw_screen (window-clock.c) starts with
# screen_write_clearscreen, so none of the pane's output shows around the
# digits. When the pane is too small for the big digits tmux writes the plain
# time centred instead. psmux cleared only the rectangle under the digits and
# drew nothing at all in a small pane.
#
# A real client in its own console; the screen is read back with
# tests\console_screen_read.cs (the overlay is drawn by the client, so
# capture-pane cannot see it).
#
# Private PSMUX_DATA_DIR and config, a -L namespace per run; cleanup is
# `-L <ns> kill-server` and the client PID this suite started.
# PSMUX_EXE (or PSMUX_TEST_BIN) picks the binary.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_BIN' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_clk_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"
$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$READ = Join-Path $TMP "screenread.exe"
if ($csc) { & $csc /nologo /optimize "/out:$READ" (Join-Path $PSScriptRoot "console_screen_read.cs") 2>&1 | Out-Null }
if (-not (Test-Path $READ)) { Write-Error "could not build tests\console_screen_read.cs"; exit 1 }
$POWERSHELL = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $POWERSHELL) { $POWERSHELL = (Get-Command powershell).Source }

$script:client = $null
$script:ns = ""
function Start-Run {
    $script:ns = "clk_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $conf = Join-Path $TMP "$($script:ns).conf"
    @("set -g default-shell '$POWERSHELL -NoLogo -NoProfile'", "set -g status off") -join "`n" | Set-Content $conf -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $conf
    & $PSMUX -L $script:ns new-session -d -s A -x 100 -y 25 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $script:ns has-session -t A 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 800
    $script:client = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $script:ns, 'attach', '-t', 'A') -PassThru
    Start-Sleep -Seconds 4
}
function Stop-Run {
    if ($script:ns) { & $PSMUX -L $script:ns kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    if ($script:client -and -not $script:client.HasExited) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
    $script:client = $null; $script:ns = ""
}
function P { & $PSMUX -L $script:ns @args 2>&1 }
# Visible rows of the client console.
function Screen {
    $f = Join-Path $TMP "screen.txt"
    & $READ $script:client.Id $f | Out-Null
    $lines = @(Get-Content $f -Encoding UTF8)
    return @($lines | Where-Object { $_.StartsWith('T|') } | ForEach-Object { $_.Substring(2) })
}
# The text of a pane's own cells on the client screen.
function PaneText($rows, $pane) {
    $g = (P display-message -p -t $pane '#{pane_left} #{pane_top} #{pane_width} #{pane_height}') -split ' '
    $l = [int]$g[0]; $t = [int]$g[1]; $w = [int]$g[2]; $h = [int]$g[3]
    $out = @()
    for ($y = $t; $y -lt $t + $h -and $y -lt $rows.Count; $y++) {
        $r = $rows[$y].PadRight($l + $w)
        $out += $r.Substring($l, $w)
    }
    return $out
}
function Fill($pane) {
    P send-keys -t $pane -l '1..60 | % { "MARK$_ " * 12 }' | Out-Null
    P send-keys -t $pane Enter | Out-Null
    Start-Sleep -Seconds 2
}

Write-Host "`n=== PR #1046 clock mode clears the whole pane ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

try {
    Start-Run
    Write-Host "`n[1] a full size pane" -ForegroundColor Yellow
    Fill 'A'
    $before = @(PaneText (Screen) 'A' | Where-Object { $_ -match 'MARK' }).Count
    if ($before -ge 20) { Write-Pass "the pane is full of output before clock mode ($before rows)" } else { Write-Fail "the pane should be full of output first, $before rows" }
    P clock-mode -t A | Out-Null
    Start-Sleep -Milliseconds 1500
    $rows = PaneText (Screen) 'A'
    $left = @($rows | Where-Object { $_ -match 'MARK' }).Count
    if ($left -eq 0) { Write-Pass "no pane output shows around the clock" } else { Write-Fail "pane output still shows on $left rows in clock mode:`n$(($rows | Where-Object { $_ -match 'MARK' } | Select-Object -First 3) -join "`n")" }
    $drawn = @($rows | Where-Object { $_.Trim() -ne '' }).Count
    if ($drawn -eq 5) { Write-Pass "only the 5 rows of big digits are drawn" } else { Write-Fail "only the 5 rows of big digits should be drawn, $drawn rows have text" }
    P send-keys -t A q | Out-Null
    Start-Sleep -Milliseconds 1000
    $back = @(PaneText (Screen) 'A' | Where-Object { $_ -match 'MARK' }).Count
    if ($back -ge 20) { Write-Pass "leaving clock mode brings the pane output back ($back rows)" } else { Write-Fail "the pane output should come back after clock mode, $back rows" }

    Write-Host "`n[2] a pane too small for the big digits" -ForegroundColor Yellow
    P split-window -v -l 3 -t A | Out-Null
    Start-Sleep -Seconds 2
    $small = (P display-message -p -t A '#{pane_id}').Trim()
    Fill $small
    $h = (P display-message -p -t $small '#{pane_height}').Trim()
    if ([int]$h -lt 5) { Write-Pass "the new pane is $h rows high" } else { Write-Fail "the new pane should be under 5 rows, got $h" }
    P clock-mode -t $small | Out-Null
    Start-Sleep -Milliseconds 1500
    $rows = PaneText (Screen) $small
    $left = @($rows | Where-Object { $_ -match 'MARK' }).Count
    if ($left -eq 0) { Write-Pass "no pane output shows in the small clock pane" } else { Write-Fail "pane output still shows on $left rows of the small clock pane" }
    $timeRow = @($rows | Where-Object { $_ -match '\b\d\d:\d\d:\d\d\b' })
    if ($timeRow.Count -eq 1) { Write-Pass "the plain time is drawn: [$($timeRow[0].Trim())]" } else { Write-Fail "the plain HH:MM:SS time should be drawn once, rows:`n$($rows -join "`n")" }
    $mid = [math]::Floor([int]$h / 2)
    if ($rows.Count -gt $mid -and $rows[$mid] -match '\d\d:\d\d:\d\d') { Write-Pass "on the middle row" } else { Write-Fail "the time should be on row $mid of the pane" }
    $other = @(PaneText (Screen) 'A:0.0' | Where-Object { $_ -match 'MARK' }).Count
    if ($other -ge 5) { Write-Pass "the other pane keeps its output ($other rows)" } else { Write-Fail "the other pane should keep its output, $other rows" }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Stop-Run
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
