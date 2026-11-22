# Issue #473: OSC 4/10/11 terminal color queries and CSI ?996n were not
# answered inside psmux, so pane applications (GitHub Copilot CLI) could not
# detect the terminal palette and fell back to wrong themes.
#
# Tests that a pane application issuing the color-query burst now receives:
#   CSI ?997;{1|2}n  (light/dark scheme)
#   OSC 10 / OSC 11  (foreground / background)
#   OSC 4;0..15      (16-color palette)
# from three color sources: Campbell fallback (no client), the server-side
# PSMUX_HOST_COLORS override, and the client-reported host-colors channel
# (attached TUI client), plus the raw TCP `host-colors` command path.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$NODE = (Get-Command node -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$repoTests = $PSScriptRoot
$PROBE = Join-Path $repoTests "color_query_probe.js"
$outDir = Join-Path $env:TEMP "psmux_test_473"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$script:TestsPassed = 0
$script:TestsFailed = 0

# Since 6b16d6d the server remembers the last palette a client reported in
# <data dir>\host_colors, so that the next server in this data root is born
# with it instead of Campbell (its spares then need no retirement on the first
# report). That memory is exactly what Part A must NOT see: it asserts the
# Campbell fallback for a server that has never had a client. Part E of this
# very suite reports Solarized Light, and on 2026-09-14 that report was written
# to the file and answered Part A in the next sweep two days later (scheme 2,
# fg 657b83, bg fdf6e3: 12 passed, 5 failed). So the file is set aside here and
# put back at the end, which also stops the suite planting a fake palette in
# the user's real data root.
$hostColorsFile = Join-Path $psmuxDir "host_colors"
$script:SavedHostColors = $null
if (Test-Path $hostColorsFile) {
    $script:SavedHostColors = Get-Content $hostColorsFile -Raw
    Remove-Item $hostColorsFile -Force -EA SilentlyContinue
}
function Restore-HostColorsFile {
    if ($null -ne $script:SavedHostColors) {
        Set-Content -Path $hostColorsFile -Value $script:SavedHostColors -NoNewline -Encoding UTF8
    } else {
        Remove-Item $hostColorsFile -Force -EA SilentlyContinue
    }
}

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Remove-Session($name) {
    & $PSMUX kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$name.*" -Force -EA SilentlyContinue
}

function Wait-Prompt($name) {
    for ($i = 0; $i -lt 40; $i++) {
        $cap = & $PSMUX capture-pane -t $name -p 2>&1 | Out-String
        if ($cap -match "PS [A-Z]:\\") { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# The Windows 10 inbox conhost (build 19045) hands psmux a query only when it
# next draws something, so there the probe writes a letter after its queries
# (PROBE_PAINT, see color_query_probe.js).
$paint = if ([Environment]::OSVersion.Version.Build -lt 22000) { "1" } else { "" }

# Runs the probe inside the session's active pane and returns the parsed JSON.
function Invoke-Probe($name, $queries = "all", $tag = "x") {
    $outFile = Join-Path $outDir "probe_${name}_${tag}.json"
    Remove-Item $outFile -Force -EA SilentlyContinue
    & $PSMUX send-keys -t $name "`$env:PROBE_OUT='$outFile'; `$env:PROBE_QUERIES='$queries'; `$env:PROBE_PAINT='$paint'; & '$NODE' '$PROBE'" Enter
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-Path $outFile) { break }
    }
    if (-not (Test-Path $outFile)) { return $null }
    Start-Sleep -Milliseconds 300
    return (Get-Content $outFile -Raw | ConvertFrom-Json)
}

function Send-TcpCommand($Session, $Command) {
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    $writer.Write("$Command`n"); $writer.Flush()
    Start-Sleep -Milliseconds 300
    $tcp.Close()
}

# Solarized Light, the exact environment from issue #473.
$SOLARIZED = "fg=657b83,bg=fdf6e3,dark=0,0=073642,1=dc322f,2=859900,3=b58900,4=268bd2,5=d33682,6=2aa198,7=eee8d5,8=002b36,9=cb4b16,10=586e75,11=657b83,12=839496,13=6c71c4,14=93a1a1,15=fdf6e3"

Write-Host "`n=== Issue #473: terminal color query responses ===" -ForegroundColor Cyan

# ── Part A: fallback colors (detached session, no client ever attached) ──
Write-Host "`n[Part A] Full burst gets Campbell fallback replies (detached)" -ForegroundColor Yellow
$S1 = "t473_fallback"
Remove-Session $S1
& $PSMUX new-session -d -s $S1
Start-Sleep -Seconds 3
if (-not (Wait-Prompt $S1)) { Write-Fail "Part A: prompt never ready"; Remove-Session $S1 }
else {
    $r = Invoke-Probe $S1 "all" "a"
    if ($null -eq $r) { Write-Fail "Part A: probe produced no result" }
    else {
        if ($r.scheme -eq 1) { Write-Pass "scheme reply = dark (997;1)" } else { Write-Fail "scheme expected 1, got: $($r.scheme)" }
        if ($r.fg -eq "rgb:cccc/cccc/cccc") { Write-Pass "fg reply = Campbell cccc" } else { Write-Fail "fg expected rgb:cccc/cccc/cccc, got: $($r.fg)" }
        if ($r.bg -eq "rgb:0c0c/0c0c/0c0c") { Write-Pass "bg reply = Campbell 0c0c" } else { Write-Fail "bg expected rgb:0c0c/0c0c/0c0c, got: $($r.bg)" }
        if ($r.paletteCount -eq 16) { Write-Pass "all 16 palette replies received" } else { Write-Fail "palette expected 16 replies, got: $($r.paletteCount)" }
        if ($r.palette.'9' -eq "rgb:e7e7/4848/5656") { Write-Pass "palette 9 = Campbell bright red" } else { Write-Fail "palette 9 expected rgb:e7e7/4848/5656, got: $($r.palette.'9')" }
    }
    # ── Part D on the same session: single-index query, no unsolicited extras ──
    Write-Host "`n[Part D] Single OSC 4;5;? gets exactly one reply, no fg/bg extras" -ForegroundColor Yellow
    $r = Invoke-Probe $S1 "idx5" "d"
    if ($null -eq $r) { Write-Fail "Part D: probe produced no result" }
    else {
        if ($r.paletteCount -eq 1 -and $r.palette.'5') { Write-Pass "exactly one palette reply (index 5)" } else { Write-Fail "expected only index 5, got count=$($r.paletteCount)" }
        if ($null -eq $r.fg -and $null -eq $r.bg) { Write-Pass "no unsolicited fg/bg replies" } else { Write-Fail "unexpected fg/bg: fg=$($r.fg) bg=$($r.bg)" }
    }
    # ── scheme-only query ──
    Write-Host "`n[Part D2] Scheme-only query (CSI ?996n)" -ForegroundColor Yellow
    $r = Invoke-Probe $S1 "scheme" "d2"
    if ($null -ne $r -and $r.scheme -eq 1 -and $r.paletteCount -eq 0) { Write-Pass "?996n alone answered with 997;1, nothing else" }
    else { Write-Fail "scheme-only query: got scheme=$($r.scheme) paletteCount=$($r.paletteCount)" }

    # ── Part C: host-colors TCP command updates the palette live ──
    Write-Host "`n[Part C] host-colors TCP command changes reported colors" -ForegroundColor Yellow
    Send-TcpCommand $S1 "host-colors $SOLARIZED"
    Start-Sleep -Milliseconds 500
    $r = Invoke-Probe $S1 "all" "c"
    if ($null -eq $r) { Write-Fail "Part C: probe produced no result" }
    else {
        if ($r.scheme -eq 2) { Write-Pass "scheme flipped to light (997;2)" } else { Write-Fail "scheme expected 2, got: $($r.scheme)" }
        if ($r.fg -eq "rgb:6565/7b7b/8383") { Write-Pass "fg = Solarized base00" } else { Write-Fail "fg expected rgb:6565/7b7b/8383, got: $($r.fg)" }
        if ($r.bg -eq "rgb:fdfd/f6f6/e3e3") { Write-Pass "bg = Solarized base3" } else { Write-Fail "bg expected rgb:fdfd/f6f6/e3e3, got: $($r.bg)" }
        if ($r.palette.'1' -eq "rgb:dcdc/3232/2f2f") { Write-Pass "palette 1 = Solarized red" } else { Write-Fail "palette 1 expected rgb:dcdc/3232/2f2f, got: $($r.palette.'1')" }
        if ($r.paletteCount -eq 16) { Write-Pass "all 16 palette replies received" } else { Write-Fail "palette expected 16, got: $($r.paletteCount)" }
    }
    Remove-Session $S1
}

# ── Part B: PSMUX_HOST_COLORS server-side override ──
Write-Host "`n[Part B] PSMUX_HOST_COLORS env override on the server" -ForegroundColor Yellow
$S2 = "t473_envsrv"
Remove-Session $S2
# Force a COLD server spawn: a warm-claimed server was created before the env
# var existed and would not see it (ad hoc env for detached sessions reaches
# the server only on cold spawn; attached sessions get it via the client
# channel, Part E).
Remove-Session "__warm__"
# NOTE: no -Wait here. pwsh 7's Start-Process -Wait waits for the whole
# process tree, and the cold-spawned detached SERVER is a child of the CLI,
# so -Wait would block until the server exits (i.e. forever).
$env:PSMUX_HOST_COLORS = $SOLARIZED
$env:PSMUX_NO_WARM = "1"
Start-Process -FilePath $PSMUX -ArgumentList "new-session","-d","-s",$S2 -WindowStyle Hidden
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 250
    if (Test-Path "$psmuxDir\$S2.port") { break }
}
$env:PSMUX_HOST_COLORS = $null
$env:PSMUX_NO_WARM = $null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $S2 2>$null
if ($LASTEXITCODE -ne 0 -or -not (Wait-Prompt $S2)) { Write-Fail "Part B: session/prompt never ready" }
else {
    $r = Invoke-Probe $S2 "all" "b"
    if ($null -eq $r) { Write-Fail "Part B: probe produced no result" }
    else {
        if ($r.scheme -eq 2 -and $r.fg -eq "rgb:6565/7b7b/8383" -and $r.bg -eq "rgb:fdfd/f6f6/e3e3" -and $r.paletteCount -eq 16) {
            Write-Pass "env override colors served (light scheme, Solarized fg/bg, 16 palette)"
        } else {
            Write-Fail "env override: scheme=$($r.scheme) fg=$($r.fg) bg=$($r.bg) count=$($r.paletteCount)"
        }
    }
}
Remove-Session $S2

# ── Part E: TUI client attach reports host colors to the server ──
Write-Host "`n[Part E] Win32 TUI: attached client reports host colors over the wire" -ForegroundColor Yellow
$S3 = "t473_tui"
Remove-Session $S3
# Server spawned WITHOUT the override — only the attaching client has it, so
# the colors must travel client -> host-colors line -> server -> pane reply.
& $PSMUX new-session -d -s $S3
Start-Sleep -Seconds 3
& $PSMUX has-session -t $S3 2>$null
if ($LASTEXITCODE -ne 0 -or -not (Wait-Prompt $S3)) { Write-Fail "Part E: session/prompt never ready" }
else {
    $env:PSMUX_HOST_COLORS = $SOLARIZED
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "attach","-t",$S3 -PassThru
    $env:PSMUX_HOST_COLORS = $null
    Start-Sleep -Seconds 4

    # TUI sanity: the attached window is functional
    $panes = (& $PSMUX display-message -t $S3 -p '#{session_attached}' 2>&1 | Out-String).Trim()
    if ($panes -match "^[1-9]") { Write-Pass "TUI: client attached (session_attached=$panes)" }
    else { Write-Fail "TUI: expected attached client, got: $panes" }

    $r = Invoke-Probe $S3 "all" "e"
    if ($null -eq $r) { Write-Fail "Part E: probe produced no result" }
    else {
        if ($r.scheme -eq 2 -and $r.fg -eq "rgb:6565/7b7b/8383" -and $r.bg -eq "rgb:fdfd/f6f6/e3e3" -and $r.paletteCount -eq 16) {
            Write-Pass "client-reported host colors served to pane app"
        } else {
            Write-Fail "client channel: scheme=$($r.scheme) fg=$($r.fg) bg=$($r.bg) count=$($r.paletteCount)"
        }
    }

    # TUI still healthy after injections: split + zoom round-trip
    & $PSMUX split-window -v -t $S3 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    $wp = (& $PSMUX display-message -t $S3 -p '#{window_panes}' 2>&1 | Out-String).Trim()
    if ($wp -eq "2") { Write-Pass "TUI: split-window works after color injections" }
    else { Write-Fail "TUI: expected 2 panes, got: $wp" }

    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}
Remove-Session $S3
Restore-HostColorsFile

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
