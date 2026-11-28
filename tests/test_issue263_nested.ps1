# Issue #263 — irrefutable proof via NESTED psmux.
#
# OUTER psmux session runs an INNER psmux session attached inside one of its
# panes. The inner psmux is fully attached, so client::render_layout_json writes
# ANSI to the outer pane's PTY.
#
# We then capture-pane on the OUTER pane, which shows literally what the inner
# psmux wrote to its parent terminal. This is the smoking gun: if the outer
# capture shows the box-drawing chars with the wrong SGR sequences, the live
# renderer is dropping per-cell colors.

$ErrorActionPreference = "Continue"
# This test's OWN pipeline decodes capture-pane's stdout (via `& $PSMUX ... |
# Out-String`) and then does a literal-character IndexOf() against the U+2502
# box-drawing glyph. Without forcing this process's own console/pipeline
# encoding to UTF-8, PowerShell decodes the external process's UTF-8 bytes
# for that multi-byte glyph using the ambient system codepage (e.g. CP437),
# mangling it into bytes that never equal the literal char below — even
# though the actual bytes captured from psmux are correct UTF-8 (verified
# independently via a raw-byte read, which shows `\e[0;90m|SGR90\e[0m` with
# the correct SGR immediately before the box char). Set it the same way the
# embedded repro script already does for its own (inner) process.
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = (Get-Command psmux -EA Stop).Source
$OUTER = "issue263_outer"
$INNER = "issue263_inner"
$psmuxDir = "$env:USERPROFILE\.psmux"
$ESC = [char]27
$BAR = [char]0x2502  # │

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# Cleanup
& $PSMUX kill-session -t $OUTER 2>&1 | Out-Null
& $PSMUX kill-session -t $INNER 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$OUTER.*" -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\$INNER.*" -Force -EA SilentlyContinue

# --- Outer psmux session (will host the inner) ---
& $PSMUX new-session -d -s $OUTER -x 200 -y 60 2>$null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $OUTER 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Outer session failed" -F Red; exit 1 }

Write-Host "`n=== Issue #263 NESTED proof ===" -ForegroundColor Cyan

# --- Write the box-drawing repro script (UTF-8 with BOM, exact issue script) ---
$reproPath = "$env:TEMP\psmux_issue263_repro.ps1"
$bar = $BAR
$reproContent = @"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "${ESC}[90m${bar} SGR 90${ESC}[0m"
Write-Host "${ESC}[37m${bar} SGR 37${ESC}[0m"
Write-Host "${ESC}[1;37m${bar} SGR 1;37${ESC}[0m"
Write-Host "${ESC}[38;5;240m${bar} 256-240${ESC}[0m"
Write-Host "${ESC}[38;5;250m${bar} 256-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128m${bar} TC grey${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0m${bar} TC red${ESC}[0m"
"@
[System.IO.File]::WriteAllText($reproPath, $reproContent, (New-Object System.Text.UTF8Encoding($true)))

Write-Info "Repro script: $reproPath"
Write-Info "Verifying script content..."
$bytes = [System.IO.File]::ReadAllBytes($reproPath)
$preview = ($bytes[0..50] | ForEach-Object { $_.ToString("X2") }) -join ' '
Write-Info "First 50 bytes: $preview"

# --- Start INNER psmux INSIDE outer (so inner is attached to outer's pane) ---
# The inner psmux treats the outer pane PTY as its terminal, so its client
# rendering lands in the outer screen buffer.
& $PSMUX send-keys -t $OUTER 'Clear-Host' Enter
Start-Sleep -Seconds 1

# Start inner attached. Use TERM=xterm-256color same as the issue env.
& $PSMUX send-keys -t $OUTER "`$env:TERM='xterm-256color'" Enter
Start-Sleep -Milliseconds 500

# Important: inner psmux must attach (not detach), otherwise we capture only PowerShell echo
& $PSMUX send-keys -t $OUTER "psmux new-session -s $INNER" Enter
Write-Info "Started inner psmux attached inside outer pane. Waiting 6s..."
Start-Sleep -Seconds 6

# Now we should be inside inner psmux's shell. Run the repro
& $PSMUX send-keys -t $OUTER "Clear-Host" Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $OUTER "& '$reproPath'" Enter
Write-Info "Ran repro inside inner psmux. Waiting 4s for render..."
Start-Sleep -Seconds 4

# --- Now capture OUTER pane: this is what the INNER psmux's renderer wrote ---
$capOuter = & $PSMUX capture-pane -t $OUTER -p -e 2>&1 | Out-String

Write-Host "`n--- OUTER capture (what inner psmux's RENDERER wrote) ---"
$capOuter -split "`n" | ForEach-Object {
    if ($_ -match "SGR " -or $_ -match "256-" -or $_ -match "TC ") {
        $shown = $_ -replace $ESC.ToString(), '\e'
        Write-Host "    $shown"
    }
}

# --- Also capture INNER pane directly (for comparison) ---
$capInner = & $PSMUX capture-pane -t $INNER -p -e 2>&1 | Out-String
Write-Host "`n--- INNER capture-pane (screen buffer state) ---"
$capInner -split "`n" | ForEach-Object {
    if ($_ -match "SGR " -or $_ -match "256-" -or $_ -match "TC ") {
        $shown = $_ -replace $ESC.ToString(), '\e'
        Write-Host "    $shown"
    }
}

# --- Compare per line: does the OUTER (live render output) carry expected SGR before each box char? ---
Write-Host "`n--- Verification: does live render keep SGR on box-drawing chars? ---"

# On Windows 10 (build 19045) the inbox pseudo console that hosts the pane
# re-encodes these colours before psmux reads them (measured there): 37 is
# its default foreground, so it writes no colour at all; bold brightens
# (1;37 comes out as 1;97); and a 256 colour index becomes the RGB of that
# index (38;5;240 as 38;2;88;88;88). psmux keeps what reaches it, so there
# the box must carry that host's spelling, the same as the text. An
# OpenConsole through PSMUX_CONPTY_DIR passes them unchanged.
$Win10Inbox = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
function Get-HostSgr([string]$sgr) {
    if (-not $Win10Inbox) { return $sgr }
    switch ($sgr) {
        '37' { '0' }
        '1;37' { '1;97' }
        '38;5;196' { '38;2;255;0;0' }
        '38;5;240' { '38;2;88;88;88' }
        '38;5;250' { '38;2;188;188;188' }
        default { $sgr }
    }
}
# Every part of $want is in $sgr. A $want of 0 (that host's default
# foreground) means $sgr only resets.
function Test-SgrHas([string]$sgr, [string]$want) {
    $parts = @($sgr -split ';' | Where-Object { $_ -ne '' })
    if ($want -eq '0') { return @($parts | Where-Object { $_ -ne '0' }).Count -eq 0 }
    foreach ($wp in $want -split ';') { if ($parts -notcontains $wp) { return $false } }
    return $true
}

$expected = @(
    @{ desc="SGR 90";   want="90";        token="SGR 90" },
    @{ desc="SGR 37";   want="37";        token="SGR 37" },
    @{ desc="SGR 1;37"; want="1;37";      token="SGR 1;37" },
    @{ desc="256-240";  want="38;5;240";  token="256-240" },
    @{ desc="256-250";  want="38;5;250";  token="256-250" },
    @{ desc="TC grey";  want="38;2;128;128;128"; token="TC grey" },
    @{ desc="TC red";   want="38;2;255;0;0";     token="TC red" }
)

$capLines = $capOuter -split "`n"

$pass = 0; $fail = 0; $missing = 0
foreach ($e in $expected) {
    $matchLine = $capLines | Where-Object { $_ -match [regex]::Escape($e.token) } | Select-Object -First 1
    if (-not $matchLine) {
        Write-Fail "$($e.desc): line containing '$($e.token)' NOT in outer capture"
        $missing++
        continue
    }
    $shown = $matchLine -replace $ESC.ToString(), '\e'

    # Get all SGRs that appear BEFORE the box-drawing char, OR if box char missing in line, look for closest SGR
    $barIndex = $matchLine.IndexOf($BAR)
    if ($barIndex -lt 0) {
        Write-Fail "$($e.desc): no $BAR char in line — box-drawing char never made it. Line=$shown"
        $fail++
        continue
    }
    $beforeBar = $matchLine.Substring(0, $barIndex)
    $sgrRx = [regex]::new("$ESC\[([^m]*)m")
    # Force an array with @(...): when Matches() returns exactly one hit,
    # `| ForEach-Object` unwraps the pipeline to a bare string scalar instead
    # of a 1-element array. Indexing a *string* with [-1] returns its last
    # CHARACTER (e.g. "0;90"[-1] -> '0'), not "the last regex match" — so
    # every single-match case silently compared against a stray trailing
    # digit instead of the real SGR value. That, not a rendering defect,
    # is why this looked like a live-render color bug (issue #263 DECIDE).
    $sgrs = @($sgrRx.Matches($beforeBar) | ForEach-Object { $_.Groups[1].Value })
    if (-not $sgrs) { $sgrs = @() }
    $lastSgr = if ($sgrs.Count -gt 0) { $sgrs[-1] } else { "(none)" }

    # Want all components present in the LAST SGR before bar
    $want = Get-HostSgr $e.want
    $allFound = Test-SgrHas $lastSgr $want
    if ($allFound) {
        Write-Pass "$($e.desc): box preceded by SGR [$lastSgr] (contains expected '$want')"
        $pass++
    } else {
        Write-Fail "$($e.desc): expected '$want' before box, got '[$lastSgr]'. Line=$shown"
        $fail++
    }
}

# --- Cleanup: kill inner first via send-keys, then outer ---
& $PSMUX send-keys -t $OUTER "psmux kill-server" Enter
Start-Sleep -Seconds 2
& $PSMUX kill-session -t $OUTER 2>&1 | Out-Null
Remove-Item $reproPath -Force -EA SilentlyContinue

Write-Host "`n=== Verdict ===" -ForegroundColor Cyan
Write-Host "  Live-render correct (PASS): $pass" -ForegroundColor Green
Write-Host "  Live-render WRONG (FAIL):   $fail" -ForegroundColor Red
Write-Host "  Lines missing entirely:     $missing" -ForegroundColor Yellow

if ($fail -gt 0) {
    Write-Host "`n  >>> BUG CONFIRMED in live render path" -ForegroundColor Red
} elseif ($missing -gt 0) {
    Write-Host "`n  >>> INCONCLUSIVE: some lines did not render" -ForegroundColor Yellow
} else {
    Write-Host "`n  >>> NO BUG IN LIVE RENDER: SGR preserved on box-drawing chars" -ForegroundColor Green
}
exit ($fail + $missing)
