# Issue #767: floating panes (tmux new-pane).
#
#  1. prefix * opens a floating pane (tmux: bind -N 'New floating pane' * { new-pane })
#  2. a floating pane is listed by list-panes (plain, -F, -a, -s) after the
#     tiled panes, #{pane_floating_flag} is 1/0, and the focused float is the
#     active pane
#  3. with a float focused, the client cursor sits inside the float
#  4. prefix x over a float draws the kill-pane? (y/n) box, n clears it and
#     keeps the float, y kills it
#
# Layers: CLI E2E, raw TCP, attached TUI client driven by WriteConsoleInput
# (tests/injector.cs) with the client's screen and cursor read back from its
# console (tests/conread.cs -c).
#
# Isolation: a unique -L namespace, cleaned up with `-L <ns> kill-server` only.
# Binary: $env:PSMUX_EXE, else the psmux on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$NS = "a767_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
$SESSION = "flt767"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }

function P { & $PSMUX -L $NS @args 2>&1 }

function Cleanup {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

function Wait-Until([scriptblock]$cond, [int]$timeoutMs = 8000, [int]$stepMs = 150) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $v = & $cond
        if ($v) { return $v }
        Start-Sleep -Milliseconds $stepMs
    }
    return $null
}

function Get-Floats([string]$target) {
    $ds = (P dump-state -t $target) -join ""
    $m = [regex]::Matches($ds, '\{"x":(\d+),"y":(\d+),"w":(\d+),"h":(\d+),"border":"([^"]*)","focused":(true|false)')
    $out = @()
    foreach ($x in $m) {
        $out += [pscustomobject]@{
            x = [int]$x.Groups[1].Value; y = [int]$x.Groups[2].Value
            w = [int]$x.Groups[3].Value; h = [int]$x.Groups[4].Value
            border = $x.Groups[5].Value; focused = ($x.Groups[6].Value -eq "true")
        }
    }
    return ,$out
}

Write-Host "`n=== Issue #767: floating panes ===" -ForegroundColor Cyan
Write-Host "  binary: $PSMUX  namespace: $NS"

# ════════════════════════════════════════════════════════════════════
# Part A: CLI E2E
# ════════════════════════════════════════════════════════════════════
Cleanup
P new-session -d -s $SESSION -x 120 -y 30 | Out-Null
$up = Wait-Until { P has-session -t $SESSION | Out-Null; $LASTEXITCODE -eq 0 }
if (-not $up) { Write-Fail "session did not start"; Cleanup; exit 1 }

Write-Host "`n[A1] prefix * is bound to new-pane" -ForegroundColor Yellow
$keys = (P list-keys -T prefix) -join "`n"
if ($keys -match '(?m)^bind-key\s+-T prefix\s+\*\s+new-pane\s*$') { Write-Pass "list-keys shows: bind-key -T prefix * new-pane" }
else { Write-Fail "no prefix * binding in list-keys: $(($keys -split "`n" | Select-String 'new-pane') -join ' / ')" }

Write-Host "`n[A2] a window with no float" -ForegroundColor Yellow
$flag = (P display-message -t $SESSION -p '#{pane_floating_flag}') -join ""
if ($flag -eq "0") { Write-Pass "pane_floating_flag is 0 for a tiled pane" }
else { Write-Fail "pane_floating_flag expected 0, got '$flag'" }

$tiledId = ((P display-message -t $SESSION -p '#{pane_id}') -join "").Trim()
P new-pane -t $SESSION | Out-Null
$f = Wait-Until { $fl = Get-Floats $SESSION; if ($fl.Count -eq 1) { $fl } }
if (-not $f) { Write-Fail "new-pane did not create a float"; Cleanup; exit 1 }

Write-Host "`n[A3] list-panes -F lists the float after the tiled pane" -ForegroundColor Yellow
$lines = @(P list-panes -t $SESSION -F '#{pane_id}|#{pane_index}|#{pane_floating_flag}|#{pane_active}')
$floatId = $null
if ($lines.Count -eq 2) { Write-Pass "two panes listed (tiled + floating)" } else { Write-Fail "expected 2 lines, got $($lines.Count): $($lines -join ' / ')" }
if ($lines.Count -ge 1 -and $lines[0] -eq "$tiledId|0|0|0") { Write-Pass "tiled pane: index 0, floating 0, not active ($($lines[0]))" }
else { Write-Fail "tiled line expected '$tiledId|0|0|0', got '$($lines[0])'" }
if ($lines.Count -ge 2 -and $lines[1] -match '^%(\d+)\|1\|1\|1$') { $floatId = "%" + $Matches[1]; Write-Pass "float: index 1, floating 1, active ($($lines[1]))" }
else { Write-Fail "float line expected '%N|1|1|1', got '$($lines[1])'" }

Write-Host "`n[A4] plain list-panes shows the float with its position" -ForegroundColor Yellow
$plain = @(P list-panes -t $SESSION | Where-Object { $_ -ne "" })
if ($plain.Count -eq 2 -and $plain[0] -notmatch '\(active\)' -and $plain[1] -match "^1: \[\d+x\d+ \d+,\d+,0\] .* $floatId \(active\)$") {
    Write-Pass "plain list-panes: '$($plain[1])'"
} else { Write-Fail "plain list-panes wrong: $($plain -join ' / ')" }

Write-Host "`n[A5] list-panes -a and -s include the float" -ForegroundColor Yellow
$all = (P list-panes -a -F '#{pane_id} #{session_name}:#{window_index}.#{pane_index} floating=#{pane_floating_flag}') -join "`n"
if ($all -match [regex]::Escape("$floatId ${SESSION}:0.1 floating=1") -and $all -match [regex]::Escape("$tiledId ${SESSION}:0.0 floating=0")) {
    Write-Pass "list-panes -a -F lists both, float flagged"
} else { Write-Fail "list-panes -a -F: $all" }
$s = (P list-panes -s -t $SESSION) -join "`n"
if ($floatId -and $s -match [regex]::Escape($floatId)) { Write-Pass "list-panes -s lists the float" } else { Write-Fail "list-panes -s: $s" }

Write-Host "`n[A6] display-message resolves the focused float as the active pane" -ForegroundColor Yellow
$dm = (P display-message -t $SESSION -p '#{pane_id} #{pane_floating_flag} #{window_panes} [#{P:#{pane_id}}]') -join ""
if ($dm -eq "$floatId 1 2 [$tiledId $floatId]") { Write-Pass "display-message: '$dm'" }
else { Write-Fail "display-message expected '$floatId 1 2 [$tiledId $floatId]', got '$dm'" }

# ════════════════════════════════════════════════════════════════════
# Part B: raw TCP
# ════════════════════════════════════════════════════════════════════
Write-Host "`n[B1] TCP list-panes -F" -ForegroundColor Yellow
$portFile = Get-ChildItem $psmuxDir -Filter "${NS}__$SESSION.port" -EA SilentlyContinue | Select-Object -First 1
$keyFile = Get-ChildItem $psmuxDir -Filter "${NS}__$SESSION.key" -EA SilentlyContinue | Select-Object -First 1
if ($portFile -and $keyFile) {
    $port = (Get-Content $portFile.FullName -Raw).Trim()
    $key = (Get-Content $keyFile.FullName -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream(); $stream.ReadTimeout = 5000
    $writer = [System.IO.StreamWriter]::new($stream); $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush(); $null = $reader.ReadLine()
    $writer.Write("list-panes -F '#{pane_id}:#{pane_floating_flag}'`n"); $writer.Flush()
    # Read lines until the server closes or goes quiet; keep what arrived.
    $resp = ""
    $stream.ReadTimeout = 2000
    while ($true) {
        try { $line = $reader.ReadLine() } catch { break }
        if ($null -eq $line) { break }
        $resp += $line + "`n"
    }
    $tcp.Close()
    if ($resp -match [regex]::Escape("${floatId}:1") -and $resp -match [regex]::Escape("${tiledId}:0")) { Write-Pass "TCP reply lists both panes: $($resp.Trim() -replace "`r?`n", ' ')" }
    else { Write-Fail "TCP reply: '$resp'" }
} else { Write-Fail "port/key file for ${NS}__$SESSION not found in $psmuxDir" }

Cleanup

# ════════════════════════════════════════════════════════════════════
# Part C: attached TUI client, real keystrokes, screen + cursor read back
# ════════════════════════════════════════════════════════════════════
Write-Host ("`n" + ("=" * 60)); Write-Host "Win32 TUI + WriteConsoleInput"; Write-Host ("=" * 60)

$toolDir = Join-Path $env:TEMP "psmux_767_tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$injector = Join-Path $toolDir "injector.exe"
$conread = Join-Path $toolDir "conread767.exe"
& $csc /nologo /optimize /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
& $csc /nologo /optimize /out:$conread "$PSScriptRoot\conread.cs" 2>&1 | Out-Null
if (-not (Test-Path $injector) -or -not (Test-Path $conread)) {
    Write-Skip "could not compile injector/conread"
} else {
    $TUI = "tui767"
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "new-session", "-s", $TUI -PassThru
    $cid = $proc.Id

    # conread occasionally returns nothing when its console attach races the
    # client's redraw; retry until it reads a screen.
    function Read-Screen {
        for ($k = 0; $k -lt 10; $k++) {
            $o = @(& $conread $cid -c 2>$null)
            if ($o.Count -gt 1) { return ,$o }
            Start-Sleep -Milliseconds 100
        }
        return ,@()
    }
    function Cursor($scr) {
        if ($scr.Count -gt 0 -and $scr[0] -match '^CURSOR x=(\d+) y=(\d+) visible=(\d)') {
            return [pscustomobject]@{ x = [int]$Matches[1]; y = [int]$Matches[2]; visible = ($Matches[3] -eq "1") }
        }
        return $null
    }

    $ready = Wait-Until { P has-session -t $TUI | Out-Null; if ($LASTEXITCODE -eq 0) { $true } } 15000
    # Wait for the client to have drawn the tiled pane's prompt.
    $drawn = Wait-Until { $s = Read-Screen; if (($s | Select-Object -Skip 1) -match 'PS [A-Z]:\\|>') { $true } } 15000
    if (-not $ready -or -not $drawn) {
        Write-Fail "TUI client did not come up"
    } else {
        Write-Host "`n[C1] prefix * opens a floating pane" -ForegroundColor Yellow
        & $injector $cid "^b{SLEEP:300}*" | Out-Null
        $fl = Wait-Until { $x = Get-Floats $TUI; if ($x.Count -eq 1) { $x } } 6000
        if ($fl -and $fl[0].focused) { Write-Pass "prefix * created a focused float at $($fl[0].x),$($fl[0].y) $($fl[0].w)x$($fl[0].h)" }
        else {
            Write-Fail "prefix * created no float"
            # Keep going with a float made from the CLI so the cursor and
            # confirmation checks still run.
            P new-pane -t $TUI | Out-Null
            $fl = Wait-Until { $x = Get-Floats $TUI; if ($x.Count -eq 1) { $x } } 6000
        }

        if ($fl) {
            $F = $fl[0]
            # Wait for the float's shell prompt before typing.
            $null = Wait-Until { (P capture-pane -t $TUI -p) -join "`n" -match 'PS [A-Z]:\\' } 15000
            Write-Host "`n[C2] the cursor is inside the float after typing into it" -ForegroundColor Yellow
            & $injector $cid "echo FLOATMARK767{ENTER}" | Out-Null
            $scr = Wait-Until {
                $s = Read-Screen
                $rows = @($s | Select-Object -Skip 1)
                $marks = @(for ($r = 0; $r -lt $rows.Count; $r++) { if ($rows[$r] -match '^\s*\S?FLOATMARK767') { $r } })
                if ($marks.Count -ge 1) { ,$s }
            } 10000
            if (-not $scr) { Write-Fail "FLOATMARK767 never echoed in the float" }
            else {
                # Let the prompt settle after the echo, then sample the cursor.
                Start-Sleep -Milliseconds 800
                $scr = Read-Screen
                $c = Cursor $scr
                $rows = @($scr | Select-Object -Skip 1)
                $markRow = -1
                for ($r = 0; $r -lt $rows.Count; $r++) { if ($rows[$r] -match '^\s*\S?FLOATMARK767') { $markRow = $r } }
                $inX = $c -and $c.x -ge ($F.x + 1) -and $c.x -le ($F.x + $F.w - 2)
                $inY = $c -and $c.y -ge ($F.y + 1) -and $c.y -le ($F.y + $F.h - 2)
                if ($c -and $c.visible -and $inX -and $inY -and $c.y -gt $markRow) {
                    Write-Pass "cursor at col $($c.x) row $($c.y), inside the float, below the echoed line (row $markRow)"
                } else {
                    Write-Fail "cursor at col $($c.x) row $($c.y) visible=$($c.visible); float spans cols $($F.x)..$($F.x + $F.w - 1) rows $($F.y)..$($F.y + $F.h - 1); echo on row $markRow"
                }
            }

            Write-Host "`n[C3] prefix x over the float draws the confirmation" -ForegroundColor Yellow
            & $injector $cid "^b{SLEEP:300}x" | Out-Null
            $shown = Wait-Until { $s = Read-Screen; if (($s | Select-Object -Skip 1) -match 'kill-pane\? \(y/n\)') { $true } } 4000
            if ($shown) { Write-Pass "kill-pane? (y/n) is on screen over the float" }
            else { Write-Fail "no kill-pane? (y/n) on screen with the float focused" }

            Write-Host "`n[C4] n clears the box and keeps the float" -ForegroundColor Yellow
            & $injector $cid "n" | Out-Null
            $cleared = Wait-Until { $s = Read-Screen; if ($s.Count -gt 1 -and -not (($s | Select-Object -Skip 1) -match 'y/n')) { $true } } 4000
            if ($cleared) { Write-Pass "confirmation box gone after n" } else { Write-Fail "confirmation box still painted after n" }
            Start-Sleep -Milliseconds 500
            $still = Get-Floats $TUI
            if ($still.Count -eq 1) { Write-Pass "float still there after n" } else { Write-Fail "float gone after n" }
            $echoN = (P capture-pane -t $TUI -p) -join "`n"
            if ($echoN -notmatch '>\s*n\s*$') { Write-Pass "the n was consumed by the confirmation, not typed into the float" }
            else { Write-Fail "the n leaked into the float" }

            Write-Host "`n[C5] prefix x then y kills the float" -ForegroundColor Yellow
            & $injector $cid "^b{SLEEP:300}x" | Out-Null
            $shown2 = Wait-Until { $s = Read-Screen; if (($s | Select-Object -Skip 1) -match 'kill-pane\? \(y/n\)') { $true } } 4000
            & $injector $cid "y" | Out-Null
            $gone = Wait-Until { $x = Get-Floats $TUI; if ($x.Count -eq 0) { $true } } 5000
            if ($shown2 -and $gone) { Write-Pass "confirmation shown, y killed the float" }
            else { Write-Fail "prefix x / y: shown=$([bool]$shown2) killed=$([bool]$gone)" }
            $tiledLeft = @(P list-panes -t $TUI -F '#{pane_floating_flag}')
            if ($tiledLeft.Count -eq 1 -and $tiledLeft[0] -eq "0") { Write-Pass "only the tiled pane remains" }
            else { Write-Fail "after kill: $($tiledLeft -join ' / ')" }

            # The box closing on n changes nothing in the server, so no new
            # frame arrives; the client must still repaint once to wipe it.
            # On a tiled pane it used to stay painted indefinitely.
            Write-Host "`n[C6] tiled pane: n wipes the kill-pane? box" -ForegroundColor Yellow
            Start-Sleep -Milliseconds 800
            & $injector $cid "^b{SLEEP:300}x" | Out-Null
            $shown3 = Wait-Until { $s = Read-Screen; if (($s | Select-Object -Skip 1) -match 'kill-pane\? \(y/n\)') { $true } } 4000
            & $injector $cid "n" | Out-Null
            $cleared3 = Wait-Until { $s = Read-Screen; if ($s.Count -gt 1 -and -not (($s | Select-Object -Skip 1) -match 'y/n')) { $true } } 4000
            if ($shown3 -and $cleared3) { Write-Pass "box shown, then wiped after n" }
            else { Write-Fail "tiled confirm: shown=$([bool]$shown3) wiped=$([bool]$cleared3)" }
            $alive = @(P list-panes -t $TUI -F '#{pane_id}')
            if ($alive.Count -eq 1) { Write-Pass "tiled pane survives n" } else { Write-Fail "tiled pane count after n: $($alive.Count)" }
        }
    }
    Cleanup
    if (-not $proc.HasExited) { Stop-Process -Id $cid -Force -EA SilentlyContinue }
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)"
exit $script:TestsFailed
