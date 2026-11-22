# Issue #662: `psmux display-message -p '#{mouse_any_flag}'` prints an empty string.
#
# The reporter asked for the six mouse tracking flags tmux publishes, so that tmux's
# own default wheel binding works unchanged in a psmux config (key-bindings.c:510):
#
#   bind -n WheelUpPane { if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' \
#       { send -M } { copy-mode -e } }
#
# MEASURED on 37e2990 (`-L i662`, a detached session):
#
#   mouse_any_flag         rc=0 -> ''
#   mouse_standard_flag    rc=0 -> ''
#   mouse_button_flag      rc=0 -> ''
#   mouse_all_flag         rc=0 -> ''
#   mouse_utf8_flag        rc=0 -> ''
#   mouse_sgr_flag         rc=0 -> ''
#   alternate_on           rc=0 -> '0'
#   pane_in_mode           rc=0 -> '0'
#
# and on this tree:
#
#   plain shell pane : 'any=0 std=0 btn=0 all=0 utf8=0 sgr=0 alt=0 mode=0 cond=0'
#   after ESC[?1000h : 'any=1 std=1 btn=0 all=0 utf8=0 sgr=1 alt=0 mode=0 cond=1'
#
# A WINDOWS FACT THIS SCRIPT DEPENDS ON.  Under ConPTY the pane's mouse state is not
# only the application's own DECSET: conhost republishes the console input mode word
# upstream, so a child that merely switches stdin to raw
# (ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS, no ENABLE_MOUSE_INPUT) makes
# psmux's parser see ESC[?1003h ESC[?1006h with nobody having asked.  Measured here:
#
#   before child     -> any=0 std=0 btn=0 all=0 utf8=0 sgr=0
#   child, no DECSET -> any=1 std=0 btn=0 all=1 utf8=0 sgr=1
#   then ESC[?1000h  -> any=1 std=1 btn=0 all=0 utf8=0 sgr=1
#
# That is why issue662_mouse_mode_child.cs sets the console mode FIRST and writes its
# DECSETs afterwards: the application's own sequences are then the last word, which is
# what a per mode measurement needs.  The `sgr` column stays 1 throughout for the same
# reason, and the assertions below never claim otherwise.
#
# Layers: format readout per mode, config acceptance of tmux's binding line verbatim,
#         and a real attached client with real MOUSE_EVENT wheel records showing that
#         the condition predicts what the wheel actually does.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }

# An attached client launched from an agent shell inherits the caller's routing
# variables and re-enters the wrong session; scrub them before Start-Process.
foreach ($v in @("PSMUX_SESSION_NAME", "PSMUX_SESSION", "PSMUX_PANE")) {
    Remove-Item "env:$v" -EA SilentlyContinue
}

$NS      = "i662"
$SESSION = "i662s"

$script:TestsPassed = 0
$script:TestsFailed = 0
# The Windows 10 inbox conhost (build 19045) keeps a pane program's mouse
# DECSET to itself, so psmux is never told about it and every flag stays 0
# there; it also drops an SGR report written to the pane, so a program that
# reads its mouse as VT bytes gets no wheel (Issue #1089, docs/faq.md).
# Measured: the DECSET 1000 to 1006 a program wrote in a pane never reached
# psmux, and SGR reports typed with send-keys -l into a raw VT reader arrived
# as nothing while the letters around them arrived.
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$child = "$env:TEMP\psmux_i662_mode_child.exe"
$inj   = "$env:TEMP\psmux_i662_injector.exe"
foreach ($pair in @(@($child, "issue662_mouse_mode_child.cs"), @($inj, "mouse_injector.cs"))) {
    & $csc /nologo /optimize "/out:$($pair[0])" (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) {
        Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red
        exit 1
    }
}

function Cleanup-All { & $PSMUX -L $NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 900 }

# The six flags of a target, read in one expansion so they describe one instant.
function Get-Flags($target) {
    $fmt = 'any=#{mouse_any_flag} std=#{mouse_standard_flag} btn=#{mouse_button_flag} ' +
           'all=#{mouse_all_flag} utf8=#{mouse_utf8_flag} sgr=#{mouse_sgr_flag} ' +
           'alt=#{alternate_on} mode=#{pane_in_mode} ' +
           'cond=#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}'
    $raw = (& $PSMUX -L $NS display-message -t $target -p $fmt 2>&1 | Out-String).Trim()
    $h = @{}
    foreach ($kv in ($raw -split '\s+')) {
        $parts = $kv -split '=', 2
        if ($parts.Count -eq 2) { $h[$parts[0]] = $parts[1] }
    }
    $h["raw"] = $raw
    return $h
}

# Start the model app in a pane and WAIT until it says it has written its
# DECSETs.  A fixed sleep raced it: on a cold first run the pane was still at a
# shell prompt six seconds in, so the flags were read before the sequence landed
# and the teardown keystroke went to the shell instead of the app.
function Start-ModeChild($target, $set) {
    $log = "$env:TEMP\psmux_i662_e2e_$($set -replace '[^0-9]','_').txt"
    Remove-Item $log -Force -EA SilentlyContinue
    $arg = if ($set) { "& '$child' set=$set log='$log'" } else { "& '$child' log='$log'" }
    & $PSMUX -L $NS send-keys -t $target $arg Enter 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        if ((Test-Path $log) -and ((Get-Content $log -EA SilentlyContinue) -contains "ARMED")) {
            Start-Sleep -Milliseconds 700   # let the server's data tick parse it
            return $log
        }
        Start-Sleep -Milliseconds 400
    }
    Write-Fail "the model app never armed for set=$set (no ARMED line in $log)"
    return $log
}

# Ask the app to withdraw every mode, and wait for the pane to agree.
function Stop-MouseModes($target) {
    & $PSMUX -L $NS send-keys -t $target -l "0" 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        $f = Get-Flags $target
        if ("$($f.any)$($f.std)$($f.btn)$($f.all)$($f.utf8)$($f.sgr)" -eq "000000") { return $f }
        Start-Sleep -Milliseconds 500
    }
    return (Get-Flags $target)
}

Write-Host "`n=== Issue #662: the six mouse tracking flags must be format variables ===" -ForegroundColor Cyan
Write-Host "  binary under test: $PSMUX" -ForegroundColor DarkGray

# ---------------------------------------------------------------------------
# Layer 1: the report itself.  Every name must render a digit, not nothing.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 1] Every flag renders 0 or 1 over a pane that asked for nothing" -ForegroundColor Yellow

Cleanup-All
& $PSMUX -L $NS new-session -d -s $SESSION -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 4
& $PSMUX -L $NS has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "could not start the detached session for the format readout"
} else {
    $names = @('mouse_any_flag','mouse_standard_flag','mouse_button_flag',
               'mouse_all_flag','mouse_utf8_flag','mouse_sgr_flag')
    foreach ($n in $names) {
        # The marker makes an empty expansion visible: '[]' is the bug, '[0]' is the fix.
        $out = (& $PSMUX -L $NS display-message -t $SESSION -p "[#{$n}]" 2>&1 | Out-String).Trim()
        if ($out -eq "[0]") {
            Write-Pass "#{$n} -> '$out'"
        } else {
            Write-Fail "#{$n} -> '$out', expected '[0]' (#662 prints '[]')"
        }
    }
    $f = Get-Flags $SESSION
    Write-Info "plain shell pane: $($f.raw)"
    if ($f.cond -eq "0") {
        Write-Pass "tmux's wheel condition is false over a plain pane, so it takes copy-mode -e"
    } else {
        Write-Fail "tmux's wheel condition is '$($f.cond)' over a plain pane, expected 0"
    }
}

# ---------------------------------------------------------------------------
# Layer 2: one DECSET at a time.  Each flag must answer for its own mode.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 2] One DECSET at a time, read back per flag" -ForegroundColor Yellow

# set -> the flag that must be 1, and the tracking flags that must be 0.
$cases = @(
    @{ Set = "1000";      On = @("std");         Off = @("btn","all") },
    @{ Set = "1002";      On = @("btn");         Off = @("std","all") },
    @{ Set = "1003";      On = @("all");         Off = @("std","btn") },
    @{ Set = "1005,1006"; On = @("utf8","sgr");  Off = @() }
)
foreach ($c in $cases) {
    Cleanup-All
    & $PSMUX -L $NS new-session -d -s $SESSION -x 100 -y 30 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    & $PSMUX -L $NS has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not come up for set=$($c.Set)"; continue }

    $null = Start-ModeChild $SESSION $c.Set
    $f = Get-Flags $SESSION
    Write-Info "child wrote ESC[?$($c.Set)h -> $($f.raw)"

    $bad = @()
    if ($Win10) {
        # This host keeps the DECSET, so every flag must stay 0.
        foreach ($k in 'any', 'std', 'btn', 'all', 'utf8', 'sgr') { if ($f[$k] -ne "0") { $bad += "$k=$($f[$k]) (want 0 on this host)" } }
    } else {
        foreach ($k in $c.On)  { if ($f[$k] -ne "1") { $bad += "$k=$($f[$k]) (want 1)" } }
        foreach ($k in $c.Off) { if ($f[$k] -ne "0") { $bad += "$k=$($f[$k]) (want 0)" } }
        if ($f.any -ne "1" -and $c.Set -ne "1005,1006") { $bad += "any=$($f.any) (want 1)" }
    }
    if ($bad.Count -eq 0) {
        Write-Pass $(if ($Win10) { "DECSET $($c.Set): every flag stays 0 on this host, which keeps the DECSET" } else { "DECSET $($c.Set) lights exactly $($c.On -join '+')" })
    } else {
        Write-Fail "DECSET $($c.Set): $($bad -join ', ')"
    }

    # And the application's own teardown must put every flag back to 0.
    $after = Stop-MouseModes $SESSION
    if ("$($after.any)$($after.std)$($after.btn)$($after.all)$($after.utf8)$($after.sgr)" -eq "000000") {
        Write-Pass "DECRST after $($c.Set) clears every flag"
    } else {
        Write-Fail "DECRST after $($c.Set) left $($after.raw)"
    }
}

# ---------------------------------------------------------------------------
# Layer 3: tmux's binding line, verbatim, in a psmux config.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 3] tmux's WheelUpPane binding in a psmux config" -ForegroundColor Yellow

$conf = "$env:TEMP\psmux_i662.conf"
Set-Content -Path $conf -Encoding ASCII -Value @(
    "bind -n WheelUpPane { if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -e } }",
    "bind -n MouseDown2Pane { select-pane -t=; if -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' { send -M } { paste -p } }"
)

Cleanup-All
& $PSMUX -L $NS new-session -d -s $SESSION -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 4
$srcOut = (& $PSMUX -L $NS source-file -t $SESSION $conf 2>&1 | Out-String).Trim()
$srcRc = $LASTEXITCODE
if ($srcRc -eq 0 -and $srcOut -eq "") {
    Write-Pass "source-file took tmux's binding lines unchanged (rc=0, no output)"
} else {
    Write-Fail "source-file rejected tmux's binding lines: rc=$srcRc out='$srcOut'"
}

$stillUp = $false
& $PSMUX -L $NS has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) { $stillUp = $true }
if ($stillUp) { Write-Pass "the server is still up after sourcing that config" }
else { Write-Fail "the server died while sourcing tmux's binding lines" }

# ---------------------------------------------------------------------------
# Layer 4: the condition has to predict what the wheel really does.  A real
# attached client, real MOUSE_EVENT records, one pane that asked for the mouse
# and one that did not, both on the MAIN screen so `mouse_any_flag` is the only
# term that can be true.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 4] The condition predicts the real wheel outcome" -ForegroundColor Yellow

Cleanup-All
$client = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 7
& $PSMUX -L $NS has-session -t $SESSION 2>$null
$clientUp = ($LASTEXITCODE -eq 0)

if (-not $clientUp) {
    Write-Fail "could not start an attached client for the wheel layer"
} else {
    & $PSMUX -L $NS set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    & $PSMUX -L $NS set-option -t $SESSION -g scroll-enter-copy-mode on 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600

    # Pane A: the shell, untouched.  Fill the scrollback so copy mode has
    # somewhere to go.
    $paneA = ((& $PSMUX -L $NS list-panes -t $SESSION -F '#{pane_id}') | Select-Object -First 1).Trim()
    & $PSMUX -L $NS send-keys -t $paneA "1..120 | ForEach-Object { `"i662 line `$_`" }" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 4

    # Pane B: a program that asks for the mouse with ESC[?1000h and nothing else.
    & $PSMUX -L $NS split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $paneB = ((& $PSMUX -L $NS list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    $logB = Start-ModeChild $paneB "1000"

    $fa = Get-Flags $paneA
    $fb = Get-Flags $paneB
    Write-Info "pane A (shell)     $($fa.raw)"
    Write-Info "pane B (ESC[?1000h) $($fb.raw)"

    if ($fa.alt -eq "0" -and $fb.alt -eq "0") {
        Write-Pass "both panes are on the main screen, so mouse_any_flag is the deciding term"
    } else {
        Write-Fail "a pane is on the alternate screen (A alt=$($fa.alt) B alt=$($fb.alt)); this layer would prove nothing"
    }
    if ($fa.cond -eq "0") { Write-Pass "the binding predicts copy-mode -e over pane A" }
    else { Write-Fail "the binding predicts '$($fa.cond)' over pane A, expected 0" }
    if ($Win10) {
        if ($fb.cond -eq "0" -and $fb.any -eq "0") { Write-Pass "on this host the binding predicts copy-mode -e over pane B too: its DECSET never reached psmux" }
        else { Write-Fail "the binding predicts '$($fb.cond)' over pane B, but this host was measured to pass no DECSET" }
    }
    elseif ($fb.cond -eq "1" -and $fb.any -eq "1") { Write-Pass "the binding predicts send -M over pane B" }
    else { Write-Fail "the binding predicts '$($fb.cond)' over pane B, expected 1" }

    function Pane-Centre($id) {
        $g = ((& $PSMUX -L $NS display-message -t $id -p '#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') 2>&1).Trim() -split '\|'
        return @{ X = [int]$g[0] + [int]([int]$g[2] / 2); Y = [int]$g[1] + [int]([int]$g[3] / 2) }
    }

    # Pane A: predicted copy mode, so copy mode is what must happen.
    $ca = Pane-Centre $paneA
    $beforeA = (& $PSMUX -L $NS display-message -t $paneA -p '#{pane_in_mode}' 2>&1).Trim()
    & $inj $client.Id up 3 $ca.X $ca.Y 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1800
    $afterA = (& $PSMUX -L $NS display-message -t $paneA -p '#{pane_in_mode}' 2>&1).Trim()
    if ($beforeA -ne "1" -and $afterA -eq "1") {
        Write-Pass "the wheel entered copy mode over pane A, as the condition said (before=$beforeA after=$afterA)"
    } else {
        Write-Fail "the wheel did not enter copy mode over pane A (before=$beforeA after=$afterA)"
    }
    & $PSMUX -L $NS send-keys -t $paneA -X cancel 2>&1 | Out-Null

    # Pane B: predicted send -M, so the application must receive an SGR report.
    $cb = Pane-Centre $paneB
    $bBefore = if (Test-Path $logB) { (Get-Content $logB).Count } else { 0 }
    & $inj $client.Id up 2 $cb.X $cb.Y 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1800
    $bAll = if (Test-Path $logB) { Get-Content $logB } else { @() }
    $bNew = if ($bAll.Count -gt $bBefore) { @($bAll[$bBefore..($bAll.Count-1)]) } else { @() }
    $got = @($bNew | Where-Object { $_ -match '<ESC>\[<6[45];' })
    $modeB = (& $PSMUX -L $NS display-message -t $paneB -p '#{pane_in_mode}' 2>&1).Trim()
    if ($Win10) {
        if ($got.Count -eq 0) { Write-Pass "on this host pane B's VT reading application got no SGR report, as measured" }
        else { Write-Fail "pane B's application got an SGR report, which this host was measured not to deliver ($($got[0]))" }
    } elseif ($got.Count -ge 1) {
        Write-Pass "the wheel reached pane B's application, as the condition said ($($got[0]))"
    } else {
        Write-Fail "the wheel did not reach pane B's application; the condition said send -M"
    }
    if ($modeB -ne "1") { Write-Pass "and pane B did not enter copy mode" }
    else { Write-Fail "pane B entered copy mode although mouse_any_flag was 1" }

    # The flags are per pane, not per window: one readout, two answers.
    $perPane = (& $PSMUX -L $NS list-panes -t $SESSION -F '#{pane_id}:#{mouse_any_flag}' 2>&1) -join " "
    $wantB = if ($Win10) { "0" } else { "1" }
    if ($perPane -match "$([regex]::Escape($paneA)):0" -and $perPane -match "$([regex]::Escape($paneB)):$wantB") {
        Write-Pass "list-panes reports the flags per pane ($perPane)"
    } else {
        Write-Fail "list-panes did not report the flags per pane ($perPane)"
    }
}

if ($client) { try { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } catch {} }
Cleanup-All

Write-Host "`n=== Issue #662 summary ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
