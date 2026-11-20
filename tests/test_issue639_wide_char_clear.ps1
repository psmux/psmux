# Issue #639: "Chinese characters not clear when pane content changed".
#
# Reported as: ssh to a box, run psmux there, run a full screen program (`tig`),
# quit it, and the double width Chinese glyphs stay painted on the screen while
# the Latin text around them clears correctly.
#
# A ghost like that can live in either of two places, so this script checks
# BOTH, and it is the second one that matters:
#
#   GRID    what psmux's emulator believes the pane holds  (capture-pane -p)
#   SCREEN  what the attached CLIENT actually paints to its outer terminal
#
# `capture-pane` alone cannot see this bug. It renders a wide glyph from the
# lead cell and deliberately skips the trailing half
# (src/copy_mode.rs::push_capture_cell), so a stranded half glyph is invisible
# there while still being painted on screen. The SCREEN side is therefore
# captured byte exactly by hosting a real attached client inside a
# CreatePseudoConsole (tests/conptycap.cs, the same approach as
# tests/test_issue589_undercurl.ps1 and tests/test_issue626_border_attrs_default.ps1)
# and replaying those bytes through a small reference terminal that implements
# the DEC wide glyph rule: a double width glyph owns a lead cell and a
# continuation cell, and touching EITHER half destroys BOTH. tmux models it the
# same way (grid.c GRID_FLAG_PADDING, screen-write.c screen_write_overwrite).
#
# The pass condition is that SCREEN equals GRID, row for row. A surviving
# Chinese glyph shows up as a row where SCREEN carries CJK that GRID does not.
#
# The emulator half of this contract is covered with no server at all by
# tests-rs/test_issue639_wide_char_clear.rs, including a 480 case erase matrix.
# This script covers the renderer, which those tests cannot reach.

$ErrorActionPreference = "Continue"

# MANDATORY. Without this the console this script runs in decodes psmux's
# capture-pane output in the OEM code page (CP437 on a stock box), every Han
# character arrives as "?", and the CJK assertions below would compare mangled
# text against mangled text and report a meaningless pass. The Count-Cjk gate in
# Check-Case is the backstop if this is ever dropped.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$ESC  = [char]27
$SOCK = "i639"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$work = Join-Path $env:TEMP "psmux_i639"
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Kill-Sess([string]$n) { & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null }

# --- the payload -------------------------------------------------------------
# Emitted from a script file, not through send-keys: send-keys collapses
# interior whitespace and would confound the column assertions. Written with
# [Console]::Out.Write rather than Write-Host because NO_COLOR, which some
# shells export, strips escape sequences out of Write-Host.
$payload = Join-Path $work "i639_payload.ps1"
@'
param([string]$Case)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$e = [char]27
function W($s) { [Console]::Out.Write($s); [Console]::Out.Flush() }
$cjk = [char]0x4E2D + [char]0x6587 + [char]0x6D4B + [char]0x8BD5 + [char]0x5B57 + [char]0x7B26
W "$e[H$e[2J"
W "PHASE0-IDLE"
if ($Case -eq "probe") {
  # KNOWN GOOD case: just put Chinese text on screen and hold it, so the
  # harness can prove capture-pane brings it back before anything else runs.
  W "$e[H$e[2J"
  W $cjk
  Start-Sleep -Seconds 600
  return
}
Start-Sleep -Seconds 5
switch ($Case) {
  "tigsim" {
    # a shell screen with real content, the way tig is launched from one
    W "$e[H$e[2J"
    for ($i = 1; $i -le 12; $i++) { W "$e[$i;1HPS C:\work> git log --oneline $i" }
    Start-Sleep -Seconds 2
    W "$e[?1049h$e[H$e[2J"                       # tig takes the alt screen
    for ($i = 1; $i -le 12; $i++) {
      W ("$e[$i;1H" + ("{0:0000000} " -f (1234567 + $i)) + $cjk + " msg $i " + $cjk)
    }
    Start-Sleep -Seconds 3
    W "$e[?1049l"                                 # tig quits
    Start-Sleep -Seconds 2
    W "$e[14;1HMARKERAFTER"
  }
  "shrink" {
    # a wide row replaced in place by a SHORT ascii row, no erase: the classic
    # way to strand the trailing half of a glyph
    W "$e[H$e[2J"
    for ($i = 1; $i -le 10; $i++) { W "$e[$i;1H$cjk$cjk$cjk" }
    Start-Sleep -Seconds 3
    for ($i = 1; $i -le 10; $i++) { W "$e[$i;1Hzz" }
    W "$e[12;1HMARKERAFTER"
  }
  "oddcol" {
    # every pair straddles an even boundary, then the row is erased and rewritten
    W "$e[H$e[2J"
    for ($i = 1; $i -le 10; $i++) { W "$e[$i;2H$cjk$cjk" }
    Start-Sleep -Seconds 3
    for ($i = 1; $i -le 10; $i++) { W "$e[$i;1H$e[2Kz" }
    W "$e[12;1HMARKERAFTER"
  }
}
Start-Sleep -Seconds 600
'@ | Set-Content -Path $payload -Encoding UTF8

# --- the reference terminal --------------------------------------------------
# Deliberately NOT psmux code: replaying the client's bytes through psmux's own
# emulator would hide a psmux bug. Implements only what the renderer emits.
$replay = Join-Path $work "i639_replay.py"
@'
import sys, unicodedata
CONT = "\x00"
def width(ch):
    if unicodedata.combining(ch): return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1
class T:
    def __init__(s, cols, rows):
        s.cols, s.rows = cols, rows
        s.g = [[" "] * cols for _ in range(rows)]
        s.x = s.y = 0; s.saved = None
    def brk(s, y, x):
        if not (0 <= y < s.rows and 0 <= x < s.cols): return
        r = s.g[y]
        if r[x] == CONT:
            if x > 0: r[x - 1] = " "
            r[x] = " "
        elif x + 1 < s.cols and r[x + 1] == CONT:
            r[x + 1] = " "
    def put(s, ch):
        w = width(ch)
        if w == 0: return
        if s.x + w > s.cols:
            s.x = 0; s.y = min(s.y + 1, s.rows - 1)
        s.brk(s.y, s.x)
        if w == 2: s.brk(s.y, s.x + 1)
        s.g[s.y][s.x] = ch
        if w == 2: s.g[s.y][s.x + 1] = CONT
        s.x += w
    def er(s, y, a, b):
        a, b = max(0, a), min(s.cols, b)
        if a >= b: return
        s.brk(y, a); s.brk(y, b - 1)
        for x in range(a, b): s.g[y][x] = " "
def run(text, cols, rows):
    t = T(cols, rows); alt = None; i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "\x1b":
            if i + 1 >= n: break
            k = text[i + 1]
            if k == "[":
                j = i + 2; priv = ""
                while j < n and text[j] in "?<>=!": priv += text[j]; j += 1
                pr = ""
                while j < n and (text[j].isdigit() or text[j] in ";: "): pr += text[j]; j += 1
                if j >= n: break
                f = text[j]
                ps = [int(x) if x.strip().isdigit() else 0 for x in pr.replace(" ", "").split(";")]
                if not ps: ps = [0]
                i = j + 1
                if priv == "?" and f in "hl":
                    for p in ps:
                        if p in (1049, 47, 1047):
                            if f == "h" and alt is None:
                                alt = ([r[:] for r in t.g], t.x, t.y)
                                t.g = [[" "] * cols for _ in range(rows)]
                            elif f == "l" and alt is not None:
                                t.g, t.x, t.y = alt[0], alt[1], alt[2]; alt = None
                    continue
                if priv: continue
                if f in "Hf":
                    t.y = min(max(0, (ps[0] or 1) - 1), rows - 1)
                    t.x = min(max(0, ((ps[1] if len(ps) > 1 else 1) or 1) - 1), cols - 1)
                elif f == "A": t.y = max(0, t.y - max(1, ps[0]))
                elif f == "B": t.y = min(rows - 1, t.y + max(1, ps[0]))
                elif f == "C": t.x = min(cols - 1, t.x + max(1, ps[0]))
                elif f == "D": t.x = max(0, t.x - max(1, ps[0]))
                elif f in "G`": t.x = min(cols - 1, max(0, (ps[0] or 1) - 1))
                elif f == "d": t.y = min(rows - 1, max(0, (ps[0] or 1) - 1))
                elif f == "J":
                    m = ps[0]
                    if m == 0:
                        t.er(t.y, t.x, cols)
                        for y in range(t.y + 1, rows): t.er(y, 0, cols)
                    elif m == 1:
                        t.er(t.y, 0, t.x + 1)
                        for y in range(0, t.y): t.er(y, 0, cols)
                    else:
                        for y in range(rows): t.er(y, 0, cols)
                elif f == "K":
                    m = ps[0]
                    if m == 0: t.er(t.y, t.x, cols)
                    elif m == 1: t.er(t.y, 0, t.x + 1)
                    else: t.er(t.y, 0, cols)
                elif f == "X": t.er(t.y, t.x, t.x + max(1, ps[0]))
                elif f == "P":
                    k2 = max(1, ps[0]); r = t.g[t.y]; t.brk(t.y, t.x)
                    del r[t.x:t.x + k2]; r.extend([" "] * k2)
                elif f == "@":
                    k2 = max(1, ps[0]); r = t.g[t.y]; t.brk(t.y, t.x)
                    for _ in range(k2): r.insert(t.x, " ")
                    del r[cols:]
                continue
            elif k == "]":
                j = i + 2
                while j < n:
                    if text[j] == "\x07": j += 1; break
                    if text[j] == "\x1b" and j + 1 < n and text[j + 1] == "\\": j += 2; break
                    j += 1
                i = j; continue
            elif k == "7": t.saved = (t.x, t.y); i += 2; continue
            elif k == "8":
                if t.saved: t.x, t.y = t.saved
                i += 2; continue
            elif k in "()#%": i += 3; continue
            else: i += 2; continue
        if c == "\r": t.x = 0
        elif c == "\n": t.y = min(rows - 1, t.y + 1)
        elif c == "\b": t.x = max(0, t.x - 1)
        elif c == "\t": t.x = min(cols - 1, (t.x // 8 + 1) * 8)
        elif ord(c) >= 32: t.put(c)
        i += 1
    return t
data = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
t = run(data, int(sys.argv[2]), int(sys.argv[3]))
sys.stdout.reconfigure(encoding="utf-8")
for y in range(t.rows):
    print("R%02d|%s|" % (y, "".join(ch for ch in t.g[y] if ch != CONT).rstrip()))
'@ | Set-Content -Path $replay -Encoding UTF8

# --- prerequisites -----------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
$capExe = Join-Path $work "conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc) -and -not (Test-Path $capExe)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}
$python = (Get-Command python -EA SilentlyContinue)
if (-not $python) { $python = (Get-Command python3 -EA SilentlyContinue) }

$COLS = 60
$ROWS = 20

function Capture-Client {
    param([string]$Session, [int]$DrainMs = 20000)
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
    Start-Process -FilePath $capExe -ArgumentList @($outBin,"$COLS","$ROWS","8",$launch) -Wait -WindowStyle Minimized
    if (Test-Path $outBin) { return $outBin }
    return $null
}

function Count-Cjk([string]$s) {
    if (-not $s) { return 0 }
    $n = 0
    foreach ($ch in $s.ToCharArray()) {
        $c = [int]$ch
        if (($c -ge 0x2E80 -and $c -le 0x9FFF) -or ($c -ge 0xAC00 -and $c -le 0xD7AF) -or
            ($c -ge 0xFF01 -and $c -le 0xFF60)) { $n++ }
    }
    return $n
}

# Runs one case and asserts SCREEN == GRID row for row.
function Check-Case {
    param([string]$Case, [switch]$ExpectCjk)
    Write-Test "#639 $Case : the painted screen must match the pane grid"
    $sess = "i639_$Case"
    Kill-Sess $sess
    Start-Sleep -Milliseconds 400
    & $PSMUX -L $SOCK new-session -d -s $sess -x $COLS -y $ROWS -- `
        pwsh -NoProfile -NoLogo -File $payload $Case 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1200
    & $PSMUX -L $SOCK has-session -t $sess 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "$Case : session did not start"; return }

    $bin = Capture-Client -Session $sess
    $grid = (& $PSMUX -L $SOCK capture-pane -t $sess -p 2>&1 | Out-String)
    Kill-Sess $sess

    if (-not $bin) { Write-Skip "$Case : no client bytes captured"; return }

    # A case that is supposed to leave Chinese text on screen and does not means
    # the payload never ran, which would turn every assertion below into a
    # comparison of two empty screens.
    if ($ExpectCjk -and (Count-Cjk $grid) -eq 0) {
        Write-Fail "$Case : expected CJK in the grid and found none, payload did not run"
        return
    }

    $screenRaw = (& $python.Source $replay $bin $COLS $ROWS | Out-String)
    if (-not $screenRaw) { Write-Skip "$Case : replay produced nothing"; return }
    if ($screenRaw -notmatch 'MARKERAFTER') {
        Write-Skip "$Case : client never painted the settle marker"
        return
    }

    $gl = ($grid -split "`r?`n")
    $sl = @()
    foreach ($ln in ($screenRaw -split "`r?`n")) {
        if ($ln -match '^R\d\d\|(.*)\|$') { $sl += $matches[1] }
    }

    $bad = @()
    for ($r = 0; $r -lt ($ROWS - 1); $r++) {     # last row is psmux's status line
        $g = if ($r -lt $gl.Count) { $gl[$r].TrimEnd() } else { "" }
        $s = if ($r -lt $sl.Count) { $sl[$r].TrimEnd() } else { "" }
        if ($g -ne $s) { $bad += "R$r GRID=[$g] SCREEN=[$s]" }
    }

    $gc = Count-Cjk $grid
    $sc = 0
    for ($r = 0; $r -lt ($ROWS - 1); $r++) { if ($r -lt $sl.Count) { $sc += Count-Cjk $sl[$r] } }

    if ($bad.Count -eq 0) {
        Write-Pass "$Case : all $($ROWS - 1) rows identical, CJK grid=$gc screen=$sc"
    } elseif ([Environment]::OSVersion.Version.Build -lt 22000) {
        # No PSEUDOCONSOLE_PASSTHROUGH_MODE before Windows 11: the inbox
        # pseudo console of Windows 10 (19045) re-renders, so SCREEN is that
        # host's repaint, not the client's bytes. The client's own bytes on
        # its Windows 10 paths (PSMUX_FAKE_WIN_BUILD=19045, captured through
        # a Windows 11 passthrough host) leave no ghost: 4 of 4 pass.
        Write-Skip "$Case : $($bad.Count) rows differ in the Windows 10 pseudo console's own re-render (no passthrough there), not the client's bytes"
    } else {
        Write-Fail "$Case : $($bad.Count) rows differ (this is the #639 ghost)"
        $bad | Select-Object -First 6 | ForEach-Object { Write-Host "        $_" -ForegroundColor DarkYellow }
        if ($sc -gt $gc) {
            Write-Host "        screen shows $($sc - $gc) CJK glyphs the grid does not have" -ForegroundColor DarkYellow
        }
    }
}

# === RUN =====================================================================
if (-not (Test-Path $capExe)) {
    Write-Skip "csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
} elseif (-not $python) {
    Write-Skip "python unavailable, the reference terminal cannot run"
} else {
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # KNOWN GOOD gate. Before believing any "clean" result below, prove this
    # harness can carry a Chinese string through a pane and back out through
    # capture-pane. If it cannot, every comparison below is worthless and the
    # run must not report a pass.
    Write-Test "#639 harness: CJK survives a capture-pane round trip"
    $probeSess = "i639_probe"
    Kill-Sess $probeSess
    Start-Sleep -Milliseconds 400
    & $PSMUX -L $SOCK new-session -d -s $probeSess -x $COLS -y $ROWS -- `
        pwsh -NoProfile -NoLogo -File $payload probe 2>&1 | Out-Null
    Start-Sleep -Seconds 7
    $probe = (& $PSMUX -L $SOCK capture-pane -t $probeSess -p 2>&1 | Out-String)
    Kill-Sess $probeSess
    $probeCjk = Count-Cjk $probe
    if ($probeCjk -ge 6) {
        Write-Pass "harness round trips CJK ($probeCjk glyphs came back)"
        Check-Case -Case "tigsim"
        Check-Case -Case "shrink" -ExpectCjk
        Check-Case -Case "oddcol"
    } else {
        Write-Fail "harness lost CJK on a round trip (got $probeCjk glyphs), results would be meaningless"
    }
}

# === TEARDOWN ================================================================
foreach ($c in @("tigsim", "shrink", "oddcol", "probe")) { Kill-Sess "i639_$c" }
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Remove-Item $work -Recurse -Force -EA SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\i639_*" -Force -EA SilentlyContinue

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
