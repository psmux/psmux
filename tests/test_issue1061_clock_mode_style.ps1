# Issue #1061: clock-mode-style. The clock of clock mode shows the time in
# the format the option names, as tmux 3.8 does (options-table.c:38-40 and
# 1330-1336, window-clock.c:249-263 at tag 3.8): `12` is `%l:%M ` and AM or
# PM, `24` is `%H:%M`, and the `-with-seconds` forms add `:%S`. A window
# value wins over the global one, and anything but the four values is
# `unknown value`.
# psmux's default stays `24-with-seconds`, the format its clock always had
# (tmux's is `24`). psmux read the option nowhere and always drew HH:MM:SS.
#
# A real client in its own console; the screen is read back with
# tests\console_screen_read.cs (the clock is drawn by the client, so
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

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cms_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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
    $script:ns = "cms_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $conf = Join-Path $TMP "$($script:ns).conf"
    # The bare shell name: a default-shell line with a spaced path and
    # arguments did not start before 805fffe4.
    @("set -g default-shell 'pwsh -NoLogo -NoProfile'", "set -g status off") -join "`n" | Set-Content $conf -Encoding ASCII
    if (-not (Get-Command pwsh -EA SilentlyContinue)) {
        @("set -g default-shell 'powershell -NoLogo -NoProfile'", "set -g status off") -join "`n" | Set-Content $conf -Encoding ASCII
    }
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
function Geometry($pane) {
    $g = (P display-message -p -t $pane '#{pane_left} #{pane_top} #{pane_width} #{pane_height}') -split ' '
    return @([int]$g[0], [int]$g[1], [int]$g[2], [int]$g[3])
}
# The text of a pane's own cells on the client screen.
function PaneText($rows, $pane) {
    $l, $t, $w, $h = Geometry $pane
    $out = @()
    for ($y = $t; $y -lt $t + $h -and $y -lt $rows.Count; $y++) {
        $r = $rows[$y].PadRight($l + $w)
        $out += $r.Substring($l, $w)
    }
    return $out
}

# tmux's text for the time `$t` (window-clock.c:249-263).
function TimeText([string]$style, [datetime]$t) {
    $h12 = $t.Hour % 12; if ($h12 -eq 0) { $h12 = 12 }
    $ampm = if ($t.Hour -ge 12) { 'PM' } else { 'AM' }
    switch ($style) {
        '12' { return ('{0,2}:{1:D2} {2}' -f $h12, $t.Minute, $ampm) }
        '12-with-seconds' { return ('{0,2}:{1:D2}:{2:D2} {3}' -f $h12, $t.Minute, $t.Second, $ampm) }
        '24' { return ('{0:D2}:{1:D2}' -f $t.Hour, $t.Minute) }
        default { return ('{0:D2}:{1:D2}:{2:D2}' -f $t.Hour, $t.Minute, $t.Second) }
    }
}

# psmux's big glyphs (src/client.rs clock_glyph), one blank column between.
$GLYPHS = @{
    '0' = @('###', '# #', '# #', '# #', '###'); '1' = @('  #', '  #', '  #', '  #', '  #')
    '2' = @('###', '  #', '###', '#  ', '###'); '3' = @('###', '  #', '###', '  #', '###')
    '4' = @('# #', '# #', '###', '  #', '  #'); '5' = @('###', '#  ', '###', '  #', '###')
    '6' = @('###', '#  ', '###', '# #', '###'); '7' = @('###', '  #', '  #', '  #', '  #')
    '8' = @('###', '# #', '###', '# #', '###'); '9' = @('###', '# #', '###', '  #', '###')
    ':' = @(' ', '#', ' ', '#', ' '); 'A' = @('###', '# #', '###', '# #', '# #')
    'P' = @('###', '# #', '###', '#  ', '#  '); 'M' = @('#   #', '## ##', '# # #', '#   #', '#   #')
    ' ' = @('   ', '   ', '   ', '   ', '   ')
}
# The 5 rows of big glyphs `$text` gives, each padded to the pane width `$w`
# with the block centred as the client centres it.
function BigRows([string]$text, [int]$w) {
    $chars = @($text.ToCharArray() | ForEach-Object { [string]$_ })
    $total = $chars.Count - 1
    foreach ($c in $chars) { $total += $GLYPHS[$c][0].Length }
    $x0 = [math]::Floor(($w - $total) / 2)
    $rows = @()
    for ($r = 0; $r -lt 5; $r++) {
        $parts = foreach ($c in $chars) { $GLYPHS[$c][$r] }
        $rows += ((' ' * $x0) + ($parts -join ' ')).TrimEnd()
    }
    return $rows
}

Write-Host "`n=== Issue #1061: clock-mode-style ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

$STYLES = @('12', '24', '12-with-seconds', '24-with-seconds')
try {
    Start-Run
    P split-window -v -l 3 -t A | Out-Null
    Start-Sleep -Seconds 2
    $big = (P display-message -p -t A:0.0 '#{pane_id}').Trim()
    $small = (P display-message -p -t A:0.1 '#{pane_id}').Trim()

    Write-Host "`n[1] the option" -ForegroundColor Yellow
    $def = (P show-options -gv clock-mode-style | Out-String).Trim()
    if ($def -eq '24-with-seconds') { Write-Pass "unset, it reads psmux's default 24-with-seconds" } else { Write-Fail "unset, it should read 24-with-seconds, got [$def]" }
    $err = (P set-option -g clock-mode-style 13 | Out-String).Trim()
    if ($err -match 'unknown value: 13') { Write-Pass "13 is refused as tmux refuses it: [$err]" } else { Write-Fail "13 should be refused with unknown value, got [$err]" }
    $after = (P show-options -gv clock-mode-style | Out-String).Trim()
    if ($after -eq '24-with-seconds') { Write-Pass "a refused value changes nothing" } else { Write-Fail "a refused value should change nothing, got [$after]" }

    Write-Host "`n[2] a pane too small for the big digits shows the text" -ForegroundColor Yellow
    foreach ($st in $STYLES) {
        P set-option -g clock-mode-style $st | Out-Null
        P clock-mode -t $small | Out-Null
        Start-Sleep -Milliseconds 1500
        # The time after the screen is read: the clock shows that second or
        # one before it.
        $rows = PaneText (Screen) $small
        $now = Get-Date
        $l, $t, $w, $h = Geometry $small
        $mid = [math]::Floor($h / 2)
        $line = if ($rows.Count -gt $mid) { $rows[$mid].TrimEnd() } else { '' }
        $ok = $false
        foreach ($when in @($now, $now.AddSeconds(-1), $now.AddSeconds(-2))) {
            $text = TimeText $st $when
            # tmux centres as width / 2 - len / 2 (window-clock.c:269).
            $want = ((' ' * ([math]::Floor($w / 2) - [math]::Floor($text.Length / 2))) + $text).TrimEnd()
            if ($line -eq $want) { $ok = $true; break }
        }
        if ($ok) { Write-Pass "$st shows [$($line.Trim())] centred on the middle row" } else { Write-Fail "$st should show [$((TimeText $st $now).Trim())] centred, the middle row is [$line]" }
        P send-keys -t $small q | Out-Null
        Start-Sleep -Milliseconds 500
    }

    Write-Host "`n[3] a full size pane shows the big digits of that text" -ForegroundColor Yellow
    foreach ($st in $STYLES) {
        P set-option -g clock-mode-style $st | Out-Null
        P clock-mode -t $big | Out-Null
        Start-Sleep -Milliseconds 1500
        $rows = @(PaneText (Screen) $big | ForEach-Object { $_.TrimEnd() })
        $now = Get-Date
        $drawn = @($rows | Where-Object { $_ -ne '' })
        $ok = $false
        foreach ($when in @($now, $now.AddSeconds(-1), $now.AddSeconds(-2))) {
            $l, $t, $w, $h = Geometry $big
            $want = BigRows (TimeText $st $when) $w
            if ($drawn.Count -eq 5 -and (($drawn -join "`n") -eq ($want -join "`n"))) { $ok = $true; break }
        }
        if ($ok) { Write-Pass "$st draws [$((TimeText $st $now).Trim())] in big digits" } else { Write-Fail "$st should draw [$((TimeText $st $now).Trim())] in big digits in the $(Geometry $big) pane, got $($drawn.Count) rows:`n$($drawn -join "`n")`nwant:`n$((BigRows (TimeText $st $now) (Geometry $big)[2]) -join "`n")" }
        P send-keys -t $big q | Out-Null
        Start-Sleep -Milliseconds 500
    }

    Write-Host "`n[4] a window value wins over the global one" -ForegroundColor Yellow
    P set-option -g clock-mode-style 24 | Out-Null
    P set-option -w -t A clock-mode-style 12 | Out-Null
    $wv = (P show-options -wv -t A clock-mode-style | Out-String).Trim()
    if ($wv -eq '12') { Write-Pass "show -w reads the window's 12" } else { Write-Fail "show -w should read 12, got [$wv]" }
    P clock-mode -t $small | Out-Null
    Start-Sleep -Milliseconds 1500
    $rows = PaneText (Screen) $small
    $now = Get-Date
    $mid = [math]::Floor($rows.Count / 2)
    $line = if ($rows.Count -gt $mid) { $rows[$mid].Trim() } else { '' }
    if ($line -eq (TimeText '12' $now).Trim()) { Write-Pass "the clock follows the window's 12: [$line]" } else { Write-Fail "the clock should follow the window's 12, got [$line]" }
    P send-keys -t $small q | Out-Null
    Start-Sleep -Milliseconds 500
    P set-option -wu -t A clock-mode-style | Out-Null
    P clock-mode -t $small | Out-Null
    Start-Sleep -Milliseconds 1500
    $rows = PaneText (Screen) $small
    $now = Get-Date
    $line = if ($rows.Count -gt $mid) { $rows[$mid].Trim() } else { '' }
    if ($line -eq (TimeText '24' $now).Trim()) { Write-Pass "with the window value gone it follows the global 24: [$line]" } else { Write-Fail "with the window value gone it should follow 24, got [$line]" }
    P send-keys -t $small q | Out-Null
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Stop-Run
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`n  Passed: $script:TestsPassed  Failed: $script:TestsFailed"
exit $script:TestsFailed
