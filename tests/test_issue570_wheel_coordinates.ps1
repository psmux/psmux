# Issue #570: mouse wheel did not reach the right window inside a TUI pane
#
# Root cause: the client sent "pane-scroll PANE up|down" with no pointer position,
# so the server reported every wheel notch at the PANE CENTRE. A TUI that routes the
# wheel by column/row (Neovim with a split, LazyVim's tree beside the editor, an
# embedded terminal) therefore always scrolled whichever of its windows happened to
# cover the centre cell, no matter where the user pointed.
#
# This test proves, at the byte level, that psmux forwards the pointer's REAL
# pane-relative column and row, and proves end to end that the correct Neovim
# window scrolls.
#
# Layers: E2E byte capture (mouse_echo_child), real console wheel injection,
#         Neovim end-to-end routing, Win32 TUI verification.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
# Honour PSMUX_DATA_DIR: the control-connection section reads $SESSION.port and
# $SESSION.key out of the data root, and hardcoding the profile made that section
# fail for anything running against a sandboxed root rather than the user's own.
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$SESSION = "test_i570"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$inj      = "$env:TEMP\psmux_mouse_injector.exe"
$vtinj    = "$env:TEMP\psmux_vt_mouse_injector.exe"
$child    = "$env:TEMP\psmux_mouse_echo_child.exe"
$childLog = "$env:TEMP\psmux_mouse_echo.txt"

foreach ($pair in @(@($inj, "mouse_injector.cs"), @($vtinj, "vt_mouse_injector.cs"), @($child, "mouse_echo_child.cs"))) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}
function Get-PaneGeom([string]$PaneId) {
    foreach ($line in (& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_right}|#{pane_width}')) {
        $p = $line -split '\|'
        if ($p[0] -eq $PaneId) { return [pscustomobject]@{ Left=[int]$p[1]; Right=[int]$p[2]; W=[int]$p[3] } }
    }
}
function Set-LeftWidth([int]$w) {
    for ($i = 0; $i -lt 15; $i++) {
        $cur = [int]((& $PSMUX display-message -t '%1' -p '#{pane_width}').Trim())
        $d = $w - $cur
        if ($d -eq 0) { break }
        if ($d -gt 0) { & $PSMUX resize-pane -t '%1' -R $d 2>&1 | Out-Null }
        else { & $PSMUX resize-pane -t '%1' -L ([Math]::Abs($d)) 2>&1 | Out-Null }
        Start-Sleep -Milliseconds 320
    }
    Start-Sleep -Milliseconds 900
}

Write-Host "`n=== Issue #570: wheel events must carry the pointer position ===" -ForegroundColor Cyan

# ─────────────────────────────────────────────────────────────────────
# PART 1: byte-level proof, native console wheel (MOUSE_EVENT records)
# ─────────────────────────────────────────────────────────────────────
Cleanup
Remove-Item $childLog -Force -EA SilentlyContinue
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 5
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }
& $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
& $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 3

$target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
& $PSMUX send-keys -t $target ($child -replace '\\','/') Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (((& $PSMUX capture-pane -t $target -p 2>&1) | Out-String) -match 'MOUSE_ECHO_READY') {
    Write-Pass "mouse-reporting child is running in pane $target"
} else {
    Write-Fail "mouse-reporting child did not start"
    Cleanup; try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}; exit 1
}

function Get-ForwardedCol([int]$ScreenX, [switch]$UseVT, [int]$ScreenY = 8) {
    $before = if (Test-Path $childLog) { (Get-Content $childLog).Count } else { 0 }
    if ($UseVT) { & $vtinj $proc.Id "\e[<64;$($ScreenX+1);$($ScreenY+1)M" 1 | Out-Null }
    else { & $inj $proc.Id "up" 1 $ScreenX $ScreenY | Out-Null }
    Start-Sleep -Milliseconds 900
    $all = if (Test-Path $childLog) { Get-Content $childLog } else { @() }
    if ($all.Count -le $before) { return $null }
    $new = $all[$before..($all.Count-1)] | Where-Object { $_ -like 'RECV*' } | Select-Object -Last 1
    if ($new -match '<ESC>\[<(\d+);(\d+);(\d+)M') {
        return [pscustomobject]@{ Btn=[int]$Matches[1]; Col=[int]$Matches[2]; Row=[int]$Matches[3] }
    }
    return $null
}

Write-Host "`n[Test 1] Native wheel reports the real column at several split widths" -ForegroundColor Yellow
foreach ($leftW in 59, 83, 36) {
    Set-LeftWidth $leftW
    $g = Get-PaneGeom $target
    if (-not $g) { Write-Fail "pane geometry unavailable at leftW=$leftW"; continue }
    $distinct = @()
    foreach ($rel in 0, [int]($g.W / 2), ($g.W - 1)) {
        $x = $g.Left + $rel
        $got = Get-ForwardedCol -ScreenX $x
        if ($null -eq $got) { Write-Fail "no wheel bytes forwarded (leftW=$leftW, X=$x)"; continue }
        $expect = $rel + 1
        if ($got.Col -eq $expect) { Write-Pass "leftW=$leftW X=$x -> col $($got.Col) (expected $expect)" }
        else { Write-Fail "leftW=$leftW X=$x -> col $($got.Col), expected $expect (pane centre bug #570)" }
        $distinct += $got.Col
    }
    if (($distinct | Select-Object -Unique).Count -gt 1) {
        Write-Pass "leftW=$leftW columns vary with pointer position"
    } else {
        Write-Fail "leftW=$leftW every notch reported the SAME column ($($distinct -join ',')) - the #570 regression"
    }
}

Write-Host "`n[Test 2] Wheel row tracks the pointer row" -ForegroundColor Yellow
$g = Get-PaneGeom $target
$rows = @()
foreach ($sy in 3, 12) {
    $got = Get-ForwardedCol -ScreenX ($g.Left + 2) -ScreenY $sy
    if ($null -ne $got) { $rows += $got.Row }
}
if ($rows.Count -eq 2 -and $rows[0] -ne $rows[1]) { Write-Pass "row varies with pointer row ($($rows -join ' vs '))" }
else { Write-Fail "row did not track the pointer (got: $($rows -join ','))" }

Write-Host "`n[Test 3] Wheel-down carries the position too (SGR button 65)" -ForegroundColor Yellow
$g = Get-PaneGeom $target
$x = $g.Left + [int]($g.W / 3)
$before = if (Test-Path $childLog) { (Get-Content $childLog).Count } else { 0 }
& $inj $proc.Id "down" 1 $x 6 | Out-Null
Start-Sleep -Milliseconds 900
$all = Get-Content $childLog
$line = if ($all.Count -gt $before) { $all[$before..($all.Count-1)] | Where-Object { $_ -like 'RECV*' } | Select-Object -Last 1 } else { "" }
if ($line -match '<ESC>\[<65;(\d+);(\d+)M') {
    $col = [int]$Matches[1]
    if ($col -eq ([int]($g.W / 3) + 1)) { Write-Pass "wheel-down reported col $col" }
    else { Write-Fail "wheel-down reported col $col, expected $([int]($g.W/3)+1)" }
} else { Write-Fail "no wheel-down bytes forwarded (got: $line)" }

Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# ─────────────────────────────────────────────────────────────────────
# PART 2: byte-level proof on the VT/SGR input path (Windows Terminal)
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 4] VT/SGR input path reports the real column" -ForegroundColor Yellow
Remove-Item $childLog -Force -EA SilentlyContinue
$env:TERM_PROGRAM = "WezTerm"
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 5
Remove-Item Env:\TERM_PROGRAM -EA SilentlyContinue
& $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
& $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 3
$target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
& $PSMUX send-keys -t $target ($child -replace '\\','/') Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4

$g = Get-PaneGeom $target
$seen = @()
foreach ($rel in 1, [int]($g.W / 2), ($g.W - 2)) {
    $got = Get-ForwardedCol -ScreenX ($g.Left + $rel) -UseVT
    if ($null -eq $got) { Write-Fail "VT path: no bytes forwarded at rel=$rel"; continue }
    if ($got.Col -eq ($rel + 1)) { Write-Pass "VT path: rel $rel -> col $($got.Col)" }
    else { Write-Fail "VT path: rel $rel -> col $($got.Col), expected $($rel + 1)" }
    $seen += $got.Col
}
if (($seen | Select-Object -Unique).Count -eq $seen.Count -and $seen.Count -gt 1) { Write-Pass "VT path: every position distinct" }
else { Write-Fail "VT path: positions collapsed to $($seen -join ',')" }

# Clicks always carried the pointer position; make sure the wheel fix did not
# disturb them. Injected as SGR because this session is on the VT input path.
Write-Host "`n[Test 5] Clicks keep their exact coordinates (no regression)" -ForegroundColor Yellow
$g = Get-PaneGeom $target
foreach ($rel in 2, [int]($g.W / 2)) {
    $x = $g.Left + $rel
    $before = (Get-Content $childLog).Count
    & $vtinj $proc.Id "\e[<0;$($x+1);7M" 1 | Out-Null
    Start-Sleep -Milliseconds 600
    $all = Get-Content $childLog
    $line = if ($all.Count -gt $before) { $all[$before..($all.Count-1)] | Where-Object { $_ -like 'RECV*' } | Select-Object -First 1 } else { "" }
    if ($line -match '\[<0;(\d+);(\d+)M' -and [int]$Matches[1] -eq ($rel + 1)) { Write-Pass "click at rel $rel reported col $($Matches[1])" }
    else { Write-Fail "click at rel $rel wrong or missing (expected col $($rel + 1), got: $line)" }
}

Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# ─────────────────────────────────────────────────────────────────────
# PART 3: end to end with Neovim (the reporter's application)
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 6] Neovim scrolls the window under the pointer, not a fixed one" -ForegroundColor Yellow
$nvimExe = Get-Command nvim -EA SilentlyContinue
if (-not $nvimExe) {
    Write-Host "  [SKIP] nvim not installed" -ForegroundColor DarkYellow
} else {
    $fileA = "$env:TEMP\i570_A.txt"; 1..500 | ForEach-Object { "AAA$_" } | Set-Content $fileA -Encoding ASCII
    $fileB = "$env:TEMP\i570_B.txt"; 1..500 | ForEach-Object { "BBB$_" } | Set-Content $fileB -Encoding ASCII
    $initVim = "$env:TEMP\i570_init.vim"
    "set mouse=a`nset noswapfile`nset shortmess+=F" | Set-Content $initVim -Encoding ASCII

    $proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
    Start-Sleep -Seconds 5
    & $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    & $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX send-keys -t $target ("nvim -u {0} -O {1} {2}" -f ($initVim -replace '\\','/'), ($fileA -replace '\\','/'), ($fileB -replace '\\','/')) Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 8
    & $PSMUX send-keys -t $target ":windo normal! 250G" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    function Get-Tops {
        $a = -1; $b = -1
        foreach ($l in (& $PSMUX capture-pane -t $target -p 2>&1)) {
            if ($a -lt 0 -and $l -match 'AAA(\d+)') { $a = [int]$Matches[1] }
            if ($b -lt 0 -and $l -match 'BBB(\d+)') { $b = [int]$Matches[1] }
        }
        return @{ A = $a; B = $b }
    }
    function Get-DividerRel {
        foreach ($l in (& $PSMUX capture-pane -t $target -p 2>&1)) {
            $i = $l.IndexOf([char]0x2502)
            if ($i -ge 0 -and $l -match 'AAA' -and $l -match 'BBB') { return $i }
        }
        return -1
    }

    if ((Get-Tops).A -lt 0) {
        Write-Fail "nvim did not come up with two windows"
    } else {
        foreach ($leftW in 59, 83, 36) {
            Set-LeftWidth $leftW
            $g = Get-PaneGeom $target
            $div = Get-DividerRel
            if ($div -lt 0) { Write-Fail "nvim divider not visible at leftW=$leftW"; continue }
            foreach ($pt in @(@{W="A"; R=[int]($div/2)}, @{W="B"; R=[int]($div + ($g.W - $div)/2)})) {
                if ($pt.R -ge $g.W) { continue }
                $x = $g.Left + $pt.R
                $before = Get-Tops
                & $inj $proc.Id "up" 3 $x 8 | Out-Null
                Start-Sleep -Milliseconds 1200
                $after = Get-Tops
                $aMoved = ($after.A -ne $before.A); $bMoved = ($after.B -ne $before.B)
                $got = if ($aMoved -and -not $bMoved) { "A" } elseif ($bMoved -and -not $aMoved) { "B" } elseif ($aMoved -and $bMoved) { "BOTH" } else { "NONE" }
                if ($got -eq $pt.W) { Write-Pass "leftW=${leftW}: wheel over nvim window $($pt.W) scrolled $got" }
                else { Write-Fail "leftW=${leftW}: wheel over nvim window $($pt.W) scrolled $got instead" }
                & $inj $proc.Id "down" 3 $x 8 | Out-Null
                Start-Sleep -Milliseconds 700
            }
        }
    }
    Cleanup
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# ─────────────────────────────────────────────────────────────────────
# PART 4: Win32 TUI verification + old-client wire compatibility
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 7] Win32 TUI still functional; 2-arg pane-scroll still accepted" -ForegroundColor Yellow
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 5
& $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
& $PSMUX split-window -v -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2
$panes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1).Trim()
if ($panes -eq "2") { Write-Pass "TUI: split-window produced 2 panes" } else { Write-Fail "TUI: expected 2 panes, got $panes" }

& $PSMUX send-keys -t $SESSION '1..200 | % { "SCROLLBACK_$_" }' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key  = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
$paneNum = 0
$pid0 = (& $PSMUX display-message -t $SESSION -p '#{pane_id}' 2>&1).Trim()
if ($pid0 -match '%(\d+)') { $paneNum = [int]$Matches[1] }

$tcp = [System.Net.Sockets.TcpClient]::new(); $tcp.NoDelay = $true
$tcp.Connect("127.0.0.1", [int]$port)
$w = [System.IO.StreamWriter]::new($tcp.GetStream()); $w.AutoFlush = $true
$w.WriteLine("AUTH $key"); $w.WriteLine("PERSISTENT"); Start-Sleep -Milliseconds 200
# legacy 2-argument form, exactly as an older client would send it
for ($i = 0; $i -lt 4; $i++) { $w.WriteLine("pane-scroll $paneNum up"); Start-Sleep -Milliseconds 60 }
Start-Sleep -Milliseconds 700
$tcp.Close()
$mode = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()
if ($mode -eq "1") { Write-Pass "legacy 2-arg pane-scroll still scrolls (entered copy mode)" }
else { Write-Fail "legacy 2-arg pane-scroll broke: pane_in_mode=$mode" }

& $PSMUX send-keys -t $SESSION -X cancel 2>&1 | Out-Null
Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
