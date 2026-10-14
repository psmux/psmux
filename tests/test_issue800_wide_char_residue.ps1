# Issue #800: a row of wide (Hangul) characters on a green background,
# rewritten by wide characters at a shifted alignment and no background, left
# green cells on the OUTER terminal while psmux's grid was clean.
#
# The bug lives in what the attached CLIENT writes to its host terminal, so this
# script hosts a real attached client inside a CreatePseudoConsole
# (tests/conptycap.cs, passthrough mode, the same harness as
# tests/test_issue639_wide_char_clear.ps1), records every byte it writes, and
# replays those bytes through a small reference terminal that tracks a
# background colour per column. The model paints a printed cell's background
# over the columns its symbol covers and touches nothing else, which is what
# Windows Terminal does when a new wide character overwrites only the left half
# of an old one (the right half keeps its colour, see the screenshot in #800).
#
# Pass condition, per variant row:
#   GRID    capture-pane -e shows no background on the row
#   SCREEN  the replayed client output leaves no green cell on the row
# plus a known good gate: frame 1 really painted green on the screen, otherwise
# a clean frame 2 proves nothing.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = $env:PSMUX_TEST_BIN }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$SOCK = "i800_" + [Guid]::NewGuid().ToString("N").Substring(0, 8)
$work = Join-Path $env:TEMP "psmux_$SOCK"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$COLS = 60
$ROWS = 20
$GoFile = Join-Path $work "go"

# --- the payload (the #800 ResidueRepro.ps1, without the key waits) ---------
# Hangul is built from code points so this file stays ASCII.
$payload = Join-Path $work "i800_payload.ps1"
@'
param([string]$Go)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$e = [char]27
function W($s) { [Console]::Out.Write($s); [Console]::Out.Flush() }
function Hangul([int[]]$cps) { -join ($cps | ForEach-Object { [char]$_ }) }
$Width = 52
$sy = @(0xAC00,0xB098,0xB2E4,0xB77C,0xB9C8,0xBC14,0xC0AC,0xC544,0xC790,0xCC28,0xCE74,0xD0C0,0xD30C,0xD558,0xAC00,0xB098)
$Old = '+ ' + (Hangul (0..24 | ForEach-Object { 0xAC74 + $_ * 97 }))
$NewKorean = ($sy | ForEach-Object { [string][char]$_ }) -join ' '
$NewAscii  = 'ab cd ef gh ij kl mn op qr st uv wx yz ab cd ef gh'
$NewWide   = '+' + $NewKorean
function Pad([string]$t) {
  $n = 0
  foreach ($c in $t.ToCharArray()) { $n += if ([int]$c -ge 0xAC00 -and [int]$c -le 0xD7A3) { 2 } else { 1 } }
  $t + (' ' * [Math]::Max(0, $Width - $n))
}
$Variants = @(
  @{ L = 'A'; Erase = '';       Text = $NewKorean },
  @{ L = 'B'; Erase = "$e[2K";  Text = $NewKorean },
  @{ L = 'C'; Erase = '';       Text = $NewAscii },
  @{ L = 'D'; Erase = '';       Text = $NewWide }
)
W "$e[?25l$e[H$e[2J"
# Frame 1 waits for the harness: the client must be attached and painting
# before it, or the client only ever sees the final grid.
for ($t = 0; $t -lt 600 -and -not (Test-Path $Go); $t++) { Start-Sleep -Milliseconds 100 }
W "$e[1;1HFRAME1"
for ($i = 0; $i -lt 4; $i++) {
  foreach ($o in 0, 1) { W ("$e[$(3 + $i * 3 + $o);1H$e[38;5;231;48;5;22m" + (Pad $Old) + "$e[0m") }
}
Start-Sleep -Milliseconds 1500
W "$e[1;1HFRAME2$e[K"
for ($i = 0; $i -lt 4; $i++) {
  $v = $Variants[$i]
  foreach ($o in 0, 1) { W ("$e[$(3 + $i * 3 + $o);1H" + $v.Erase + (Pad $v.Text)) }
  W "$e[$(3 + $i * 3 + 2);1HROW$($v.L)"
}
W "$e[16;1HMARKERAFTER"
Start-Sleep -Seconds 600
'@ | Set-Content -Path $payload -Encoding UTF8

# --- the reference terminal: one background colour per column ---------------
$replay = Join-Path $work "i800_replay.py"
@'
import sys, unicodedata
def width(ch):
    if unicodedata.combining(ch) or ch in "\u200b\u200d\ufe0f\ufe0e": return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1
GREEN = "5;22"
def sgr(ps, bg):
    i = 0
    while i < len(ps):
        p = ps[i]
        if p in ("", "0"): bg = None
        elif p == "49": bg = None
        elif p in ("38", "48"):
            sub = ps[i + 1] if i + 1 < len(ps) else ""
            if sub == "5":
                if p == "48": bg = "5;" + ps[i + 2]
                i += 2
            elif sub == "2":
                if p == "48": bg = "2;" + ";".join(ps[i + 2:i + 5])
                i += 4
        elif p.startswith("48:"):
            bg = p[3:].replace(":", ";")
        elif len(p) == 2 and p[0] == "4" and p[1] in "01234567": bg = "c" + p
        elif len(p) == 3 and p[:2] == "10": bg = "c" + p
        i += 1
    return bg
data = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
cols, rows = int(sys.argv[2]), int(sys.argv[3])
bgc = [[None] * cols for _ in range(rows)]
txt = [[" "] * cols for _ in range(rows)]
x = y = 0; bg = None; ever = 0; i = 0; n = len(data)
def erase(yy, a, b):
    for xx in range(max(0, a), min(cols, b)): bgc[yy][xx] = bg; txt[yy][xx] = " "
while i < n:
    c = data[i]
    if c == "\x1b":
        if i + 1 >= n: break
        k = data[i + 1]
        if k == "[":
            j = i + 2; priv = ""
            while j < n and data[j] in "?<>=!": priv += data[j]; j += 1
            pr = ""
            while j < n and (data[j].isdigit() or data[j] in ";: "): pr += data[j]; j += 1
            if j >= n: break
            f = data[j]; i = j + 1
            if priv: continue
            ps = pr.split(";")
            num = [int(v) if v.isdigit() else 0 for v in ps]
            if f == "m": bg = sgr(ps, bg)
            elif f in "Hf":
                y = min(max(0, (num[0] or 1) - 1), rows - 1)
                x = min(max(0, ((num[1] if len(num) > 1 else 1) or 1) - 1), cols - 1)
            elif f == "A": y = max(0, y - max(1, num[0]))
            elif f == "B": y = min(rows - 1, y + max(1, num[0]))
            elif f == "C": x = min(cols - 1, x + max(1, num[0]))
            elif f == "D": x = max(0, x - max(1, num[0]))
            elif f in "G`": x = min(cols - 1, max(0, (num[0] or 1) - 1))
            elif f == "d": y = min(rows - 1, max(0, (num[0] or 1) - 1))
            elif f == "J":
                if num[0] == 0:
                    erase(y, x, cols)
                    for yy in range(y + 1, rows): erase(yy, 0, cols)
                elif num[0] == 1:
                    erase(y, 0, x + 1)
                    for yy in range(0, y): erase(yy, 0, cols)
                else:
                    for yy in range(rows): erase(yy, 0, cols)
            elif f == "K":
                if num[0] == 0: erase(y, x, cols)
                elif num[0] == 1: erase(y, 0, x + 1)
                else: erase(y, 0, cols)
            elif f == "X": erase(y, x, x + max(1, num[0]))
            continue
        elif k in "]P_^":
            j = i + 2
            while j < n:
                if data[j] == "\x07": j += 1; break
                if data[j] == "\x1b" and j + 1 < n and data[j + 1] == "\\": j += 2; break
                j += 1
            i = j; continue
        elif k in "()#%": i += 3; continue
        else: i += 2; continue
    if c == "\r": x = 0
    elif c == "\n": y = min(rows - 1, y + 1)
    elif c == "\b": x = max(0, x - 1)
    elif ord(c) >= 32:
        w = width(c)
        if w:
            if x + w > cols: x = 0; y = min(y + 1, rows - 1)
            for xx in range(x, min(cols, x + w)):
                bgc[y][xx] = bg
                if bg == GREEN: ever += 1
            txt[y][x] = c
            for xx in range(x + 1, min(cols, x + w)): txt[y][xx] = ""
            x = min(x + w, cols - 1) if x + w < cols else cols - 1
    i += 1
sys.stdout.reconfigure(encoding="utf-8")
print("EVER %d" % ever)
for yy in range(rows):
    g = [str(xx + 1) for xx in range(cols) if bgc[yy][xx] == GREEN]
    print("R%02d|%s|%s" % (yy, ",".join(g), "".join(txt[yy]).rstrip()))
'@ | Set-Content -Path $replay -Encoding UTF8

# --- prerequisites -----------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
$capExe = Join-Path $work "conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}
$python = (Get-Command python -EA SilentlyContinue)
if (-not $python) { $python = (Get-Command python3 -EA SilentlyContinue) }

function Capture-Client {
    param([string]$Session, [int]$DrainMs = 9000)
    $launch = Join-Path $work "attach_$Session.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $SOCK attach -t $Session
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = Join-Path $work "client_$Session.bin"
    Remove-Item $outBin -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "$DrainMs"
    $cap = Start-Process -FilePath $capExe -ArgumentList @($outBin, "$COLS", "$ROWS", "8", $launch) -WindowStyle Minimized -PassThru
    # Wait until the attached client has painted the status line, then let
    # the payload draw frame 1.
    for ($t = 0; $t -lt 100; $t++) {
        Start-Sleep -Milliseconds 100
        if ((Test-Path $outBin) -and (Get-Item $outBin).Length -gt 200) { break }
    }
    Start-Sleep -Milliseconds 800
    Set-Content -Path $GoFile -Value go
    $cap | Wait-Process -Timeout 120 -EA SilentlyContinue
    if (-not $cap.HasExited) { Stop-Process -Id $cap.Id -Force -EA SilentlyContinue }
    if (Test-Path $outBin) { return $outBin }
    return $null
}

# Variant name to pane row (0 based) of its two rewritten rows.
$VariantRows = [ordered]@{ A = @(2, 3); B = @(5, 6); C = @(8, 9); D = @(11, 12) }

if (-not (Test-Path $capExe)) {
    Write-Skip "csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
} elseif (-not $python) {
    Write-Skip "python unavailable, the reference terminal cannot run"
} else {
    $sess = "i800"
    Write-Test "#800 setup: session with the residue payload ($SOCK)"
    & $PSMUX -L $SOCK new-session -d -s $sess -x $COLS -y $ROWS -- `
        pwsh -NoProfile -NoLogo -File $payload $GoFile 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1200
    & $PSMUX -L $SOCK has-session -t $sess 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "session did not start"
    } else {
        $bin = Capture-Client -Session $sess
        $gridE = (& $PSMUX -L $SOCK capture-pane -t $sess -e -p 2>&1 | Out-String)
        $gridLines = $gridE -split "`r?`n"
        if (-not $bin) {
            Write-Skip "no client bytes captured"
        } else {
            $out = (& $python.Source $replay $bin $COLS $ROWS | Out-String)
            $lines = $out -split "`r?`n"
            $ever = 0
            $green = @{}
            $text = @{}
            foreach ($ln in $lines) {
                if ($ln -match '^EVER (\d+)') { $ever = [int]$matches[1] }
                if ($ln -match '^R(\d\d)\|([^|]*)\|(.*)$') { $green[[int]$matches[1]] = $matches[2]; $text[[int]$matches[1]] = $matches[3] }
            }
            $allText = ($text.Values -join "`n")
            Write-Test "#800 harness gates"
            if ($allText -notmatch 'MARKERAFTER') {
                Write-Skip "client never painted frame 2 (no MARKERAFTER on the replayed screen)"
            } elseif ($ever -lt 100) {
                Write-Fail "frame 1 never painted green on the screen (EVER=$ever), results would be meaningless"
            } else {
                Write-Pass "frame 1 painted $ever green cells, frame 2 reached the screen"
                foreach ($v in $VariantRows.Keys) {
                    Write-Test "#800 variant $v"
                    foreach ($r in $VariantRows[$v]) {
                        $g = $gridLines[$r]
                        if ($g -match '48;5;22|\[4[0-7]m|48;2;') {
                            Write-Fail "$v row $r GRID still carries a background: $g"
                        } else {
                            Write-Pass "$v row $r GRID (capture-pane -e) has no background"
                        }
                        if ($green[$r]) {
                            Write-Fail "$v row $r SCREEN keeps green at columns $($green[$r])"
                        } else {
                            Write-Pass "$v row $r SCREEN has no green cell"
                        }
                    }
                }
            }
        }
    }
}

# === TEARDOWN: only this test's namespace =====================================
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
if (-not $env:PSMUX_I800_KEEP) { Remove-Item $work -Recurse -Force -EA SilentlyContinue } else { Write-Host "kept $work" }

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
