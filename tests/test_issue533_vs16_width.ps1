# Issue #533: VS16 emoji-presentation sequences stored as one narrow cell.
#
# A text-presentation base character followed by U+FE0F (VS16) requests emoji
# presentation, which real terminals and tmux count as TWO columns. psmux's
# vt100 grid measured width one character at a time, so the base settled the
# cell at one column and the selector folded in as a zero-width mark. Every
# column after it drifted left by one, and the skipped cell kept the previous
# frame's content.
#
# ---------------------------------------------------------------------------
# READ THIS BEFORE TRUSTING A PASS FROM THE PANE ASSERTIONS
# ---------------------------------------------------------------------------
# The column arithmetic for this bug happens in psmux's vt100 parser ONLY when
# the child's raw bytes reach that parser. That requires ConPTY passthrough
# mode. Without it, conhost keeps its own screen model and re-serialises a
# normalised stream, so it resolves the geometry itself and hands psmux an
# already-composed row. Verified here with PSMUX_PANE_RAW=1 on Windows 11
# build 26200, where CreateProcessW rejects the passthrough HPCON with
# ERROR_INVALID_PARAMETER and portable-pty falls back to plain ConPTY:
#
#   child wrote : <ESC>[2J<ESC>[H  U+2733 U+FE0F 0123456789  <ESC>[4G X
#   parser saw  : U+2733 U+FE0F 0X23456789 <ESC>[1;5H        <- ESC[4G gone
#
# On such a build the pane can NOT discriminate this bug: the row looks right
# whether or not the fix is present, because psmux never did the arithmetic.
# This script therefore probes the raw stream first and only runs the
# discriminating assertion when passthrough actually delivered the sequence
# intact. Otherwise it says so and falls back to the assertions it CAN honestly
# make (psmux stores and returns the sequence without corrupting it).
#
# The authoritative reproduction for this bug is the crate-level suite,
# crates/vt100-psmux/tests/issue533_vs16_width.rs, which drives the parser
# directly and is exactly how the reporter's consumer uses the crate.
#
# tmux 3.4 parity oracle (cursor_x after writing the sequence):
#   U+2733 -> 1,  U+2733 U+FE0F -> 2,  U+2733 U+FE0E -> 1,  U+1F4DB -> 2
#   tmux ships variation-selector-always-wide ON by default.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_issue533"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }

function Cleanup($name) {
    & $PSMUX kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$psmuxDir\$name.*" -Force -EA SilentlyContinue
}

# Payload run INSIDE the pane, from a real child process through the PTY.
# ESC[4G is absolute column 4 (1-based) = index 3. With the emoji correctly
# occupying columns 0 and 1 the row reads E 0 1 2 ... so index 3 holds '1':
#   emoji 1 col wide -> X lands on '2' -> row is  <E>01X3456789
#   emoji 2 col wide -> X lands on '1' -> row is  <E>0X23456789
function New-Payload {
    param([string]$Path, [int]$Base, [int]$Selector)
    $sel = if ($Selector -gt 0) { "+ [string][char]$Selector" } else { "" }
    $body = @"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(`$false)
`$e = [char]27
`$g = [string][char]$Base $sel
[Console]::Out.Write("`$e[2J`$e[H")
[Console]::Out.Write(`$g + "0123456789")
[Console]::Out.Write("`$e[4GX")
[Console]::Out.Write("`$e[10;1H")
[Console]::Out.Flush()
"@
    Set-Content -Path $Path -Value $body -Encoding UTF8
}

$pVs16 = "$env:TEMP\psmux_i533_vs16.ps1"; New-Payload $pVs16 0x2733 0xFE0F
$pVs15 = "$env:TEMP\psmux_i533_vs15.ps1"; New-Payload $pVs15 0x2733 0xFE0E
$pBare = "$env:TEMP\psmux_i533_bare.ps1"; New-Payload $pBare 0x2733 0

function Invoke-PaneRow {
    param([string]$Name, [string]$Script, [switch]$LogRaw)
    Cleanup $Name
    if ($LogRaw) {
        Remove-Item "$psmuxDir\pane_raw.bin" -Force -EA SilentlyContinue
        $env:PSMUX_PANE_RAW = "1"
    }
    & $PSMUX new-session -d -s $Name 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    & $PSMUX has-session -t $Name 2>$null
    if ($LASTEXITCODE -ne 0) { $env:PSMUX_PANE_RAW = $null; return $null }
    & $PSMUX send-keys -t $Name "pwsh -NoProfile -File `"$Script`"" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 5
    $cap = & $PSMUX capture-pane -t $Name -p 2>&1 | Out-String
    $env:PSMUX_PANE_RAW = $null
    foreach ($l in ($cap -split "`r?`n")) { if ($l.Trim().Length -gt 0) { return $l } }
    return ""
}

# Reads the raw pre-parse pane stream. The file is held open by the server, so
# open it with shared read/write access.
function Get-RawPaneText {
    $f = "$psmuxDir\pane_raw.bin"
    if (-not (Test-Path $f)) { return "" }
    try {
        $fs = [System.IO.File]::Open($f, 'Open', 'Read', 'ReadWrite')
        $ms = New-Object System.IO.MemoryStream
        $fs.CopyTo($ms); $fs.Close()
        return [System.Text.Encoding]::UTF8.GetString($ms.ToArray())
    } catch { return "" }
}

Write-Host "`n=== Issue #533: VS16 emoji width E2E ===" -ForegroundColor Cyan

# === TEST 1: establish whether this build can discriminate at all ===
Write-Host "`n[Test 1] Probe the raw pre-parse pane stream" -ForegroundColor Yellow
$rowVs16 = Invoke-PaneRow -Name $SESSION -Script $pVs16 -LogRaw
$raw = Get-RawPaneText
$passthrough = $raw -match "\e\[4G"
if ($rowVs16 -eq $null) {
    Write-Fail "Session creation failed"
} elseif ($passthrough) {
    Write-Pass "ConPTY passthrough is live: the parser receives ESC[4G and does the column math"
} else {
    Write-Skip "No passthrough on this build: conhost composed the row before psmux saw it"
    Write-Info "The pane cannot discriminate #533 here. Crate suite is authoritative."
}

# === TEST 2: the discriminating column assertion (passthrough only) ===
Write-Host "`n[Test 2] VS16 sequence reserves 2 columns in a live pane" -ForegroundColor Yellow
Write-Info "row: '$($rowVs16.TrimEnd())'"
if ($passthrough) {
    if ($rowVs16 -match "0X23456789") {
        Write-Pass "Overwrite at col 3 hit '1' -> emoji occupies 2 columns (no drift)"
    } elseif ($rowVs16 -match "01X3456789") {
        Write-Fail "BUG PRESENT: overwrite at col 3 hit '2' -> emoji occupies only 1 column"
    } else {
        Write-Fail "Unrecognised row shape: '$($rowVs16.TrimEnd())'"
    }
} else {
    Write-Skip "Column geometry here is conhost's, not psmux's: asserting it would prove nothing"
}

# === TEST 3: psmux stores and returns the sequence without corrupting it ===
# This IS honest on every build: whatever geometry conhost hands over, psmux
# must round trip the base+selector pair and the whole trailing digit run.
Write-Host "`n[Test 3] Sequence survives the grid and capture-pane round trip" -ForegroundColor Yellow
$hasBase = $rowVs16 -match "`u{2733}"
$hasSel  = $rowVs16 -match "`u{FE0F}"
if ($hasBase -and $hasSel) {
    Write-Pass "capture-pane returns the base U+2733 and the U+FE0F selector intact"
} else {
    Write-Fail "Sequence mangled: base=$hasBase selector=$hasSel in '$($rowVs16.TrimEnd())'"
}
if ($rowVs16 -match "X23456789" -or $rowVs16 -match "X3456789") {
    Write-Pass "Trailing digit run intact, nothing swallowed or duplicated"
} else {
    Write-Fail "Trailing run damaged: '$($rowVs16.TrimEnd())'"
}

# === TEST 4: VS15 and the bare base must not be corrupted either ===
Write-Host "`n[Test 4] VS15 and bare base round trip cleanly" -ForegroundColor Yellow
$rowVs15 = Invoke-PaneRow -Name "test_issue533_vs15" -Script $pVs15
Write-Info "vs15 row: '$($rowVs15.TrimEnd())'"
if ($rowVs15 -match "`u{2733}" -and $rowVs15 -match "`u{FE0E}") {
    Write-Pass "VS15 sequence stored with its selector intact"
} else {
    Write-Fail "VS15 sequence mangled: '$($rowVs15.TrimEnd())'"
}
if ($passthrough) {
    if ($rowVs15 -match "01X3456789") {
        Write-Pass "VS15 stays narrow (tmux parity: cursor_x 1)"
    } else {
        Write-Fail "VS15 was promoted, tmux keeps text presentation at 1 column"
    }
} else {
    Write-Skip "VS15 column geometry is conhost's here (it renders the base wide)"
}

$rowBare = Invoke-PaneRow -Name "test_issue533_bare" -Script $pBare
Write-Info "bare row: '$($rowBare.TrimEnd())'"
if ($rowBare -match "`u{2733}") {
    Write-Pass "Bare base character round trips"
} else {
    Write-Fail "Bare base mangled: '$($rowBare.TrimEnd())'"
}

# === TEST 5: TCP server path returns the same grid ===
Write-Host "`n[Test 5] Raw TCP capture-pane agrees with the CLI" -ForegroundColor Yellow
$portFile = "$psmuxDir\$SESSION.port"
$keyFile  = "$psmuxDir\$SESSION.key"
if ((Test-Path $portFile) -and (Test-Path $keyFile)) {
    try {
        $port = (Get-Content $portFile -Raw).Trim()
        $key  = (Get-Content $keyFile -Raw).Trim()
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
        $tcp.NoDelay = $true
        $stream = $tcp.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream)
        $reader = [System.IO.StreamReader]::new($stream, [System.Text.UTF8Encoding]::new($false))
        $writer.Write("AUTH $key`n"); $writer.Flush()
        if ($reader.ReadLine() -eq "OK") {
            $writer.Write("capture-pane -p`n"); $writer.Flush()
            $stream.ReadTimeout = 5000
            $tcpRow = $null
            for ($i = 0; $i -lt 60; $i++) {
                try { $line = $reader.ReadLine() } catch { break }
                if ($null -eq $line) { break }
                if ($line -match "3456789") { $tcpRow = $line; break }
            }
            if ($tcpRow) {
                Write-Info "tcp row: '$($tcpRow.TrimEnd())'"
                if ($tcpRow.TrimEnd() -eq $rowVs16.TrimEnd()) {
                    Write-Pass "TCP path returns the identical row to the CLI path"
                } else {
                    Write-Fail "TCP row differs from CLI row"
                }
            } else {
                Write-Fail "TCP capture-pane returned no marker row"
            }
        } else {
            Write-Fail "TCP AUTH failed"
        }
        $tcp.Close()
    } catch {
        Write-Fail "TCP path error: $_"
    }
} else {
    Write-Fail "Port/key file missing for TCP test"
}

# ============================================================
# Win32 TUI VISUAL VERIFICATION (Layer 2, mandatory)
# ============================================================
Write-Host "`n$('=' * 60)" -ForegroundColor Cyan
Write-Host "Win32 TUI VISUAL VERIFICATION" -ForegroundColor Cyan
Write-Host ("=" * 60) -ForegroundColor Cyan

$SESSION_TUI = "issue533_tui_proof"
Cleanup $SESSION_TUI
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 5

& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "TUI: real attached psmux window is alive"

    & $PSMUX send-keys -t $SESSION_TUI "pwsh -NoProfile -File `"$pVs16`"" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 5
    $tuiCap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
    $tuiRow = ($tuiCap -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 } | Select-Object -First 1)
    Write-Info "TUI row: '$tuiRow'"
    if ($tuiRow -match "`u{2733}" -and $tuiRow -match "3456789") {
        Write-Pass "TUI: attached client renders the VS16 row without corruption"
    } else {
        Write-Fail "TUI: attached client row wrong: '$tuiRow'"
    }

    # Prove the TUI session is still fully functional after the emoji traffic.
    & $PSMUX split-window -v -t $SESSION_TUI 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $panes = (& $PSMUX display-message -t $SESSION_TUI -p '#{window_panes}' 2>&1).Trim()
    if ($panes -eq "2") { Write-Pass "TUI: split-window created 2 panes" }
    else { Write-Fail "TUI: expected 2 panes, got '$panes'" }

    & $PSMUX resize-pane -Z -t $SESSION_TUI 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $zoom = (& $PSMUX display-message -t $SESSION_TUI -p '#{window_zoomed_flag}' 2>&1).Trim()
    if ($zoom -eq "1") { Write-Pass "TUI: resize-pane -Z zoomed" }
    else { Write-Fail "TUI: zoom expected 1, got '$zoom'" }
} else {
    Write-Fail "TUI: attached session never came up"
}

& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# === TEARDOWN ===
Cleanup $SESSION
Cleanup "test_issue533_vs15"
Cleanup "test_issue533_bare"
Cleanup $SESSION_TUI
Remove-Item $pVs16,$pVs15,$pBare -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor Yellow
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsSkipped -gt 0) {
    Write-Host "`n  NOTE: skipped assertions need ConPTY passthrough. The parser-level" -ForegroundColor DarkYellow
    Write-Host "  proof for #533 lives in crates/vt100-psmux/tests/issue533_vs16_width.rs" -ForegroundColor DarkYellow
}
exit $script:TestsFailed
