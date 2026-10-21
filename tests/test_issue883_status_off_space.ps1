# Issue #883: turning the status line off must give its row back to the panes.
#
# The attached client reported `terminal rows - max(1, status rows)` as its
# size, so with `status off` every pane stayed one row short: the ConPTY, the
# app inside and the layout all got T-1 rows while the client drew the panes
# over all T rows, leaving the bottom row blank. tmux gives the window
# `tty.sy - status_line_size(c)` rows and that is 0 for off (status.c:113,
# resize.c:194), recalculated on every option change (options.c:1449).
#
# T is the attached window's row count, measured as pane rows + 1 with the
# status on. Checked with a real visible client:
#   1. status on: the pane has T-1 rows and #{client_height} is T,
#   2. set -g status off: the pane, the window and the ConPTY (what the shell
#      inside reads from [Console]::WindowHeight) have T rows,
#   3. a fullscreen app that writes a marker on every row gets its last
#      marker on row T (capture-pane),
#   4. status 2 / status 3 / back off: T-2, T-3, T,
#   5. per session `set status off` and `set -u status`,
#   6. two panes and zoom with the status off fill all T rows,
#   7. `set -g status off` in the config file at startup gives T rows,
#   8. a detached session keeps its -y size whatever the status (as tmux).
#
# Isolation: private PSMUX_DATA_DIR, unique -L namespace on every command,
# cleanup only with `-L <ns> kill-server`, and only windows this script
# started are closed, by PID.

$ErrorActionPreference = 'Continue'

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$ns = "t883_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_issue883_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$oldData = $env:PSMUX_DATA_DIR
$oldConf = $env:PSMUX_CONFIG_FILE
$env:PSMUX_DATA_DIR = $data
$emptyConf = Join-Path $data 'empty.conf'
Set-Content -Path $emptyConf -Value '' -Encoding UTF8
$env:PSMUX_CONFIG_FILE = $emptyConf
Remove-Item Env:\TMUX -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX_PANE -ErrorAction SilentlyContinue

$script:pass = 0; $script:fail = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:pass++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>$null }
function Fmt($t, $f) { ((P display-message -t $t -p $f) -join '').Trim() }
function PaneH($t) { [int](Fmt $t '#{pane_height}') }

# Wait until a format value stops changing and equals what is expected, or
# give up after a few seconds and return what it reads.
function WaitFor($t, $f, $want) {
    $v = $null
    for ($i = 0; $i -lt 30; $i++) {
        $v = Fmt $t $f
        if ("$v" -eq "$want") { return $v }
        Start-Sleep -Milliseconds 200
    }
    return $v
}

function WaitSession($s) {
    for ($i = 0; $i -lt 60; $i++) {
        P has-session -t $s | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function WaitAttached($s) {
    for ($i = 0; $i -lt 60; $i++) {
        if ((Fmt $s '#{session_attached}') -match '^[1-9]') { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# What the shell inside the pane sees as its console height (the ConPTY size).
function InnerRows($t) {
    P send-keys -t $t 'clear; Write-Host ("INNER=" + [Console]::WindowHeight)' Enter | Out-Null
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 250
        $m = (P capture-pane -t $t -p) -join "`n" | Select-String -Pattern 'INNER=(\d+)' -AllMatches
        if ($m) { return [int]$m.Matches[-1].Groups[1].Value }
    }
    return -1
}

# A fullscreen app: clear, then write ROW<n> on every row of its console,
# the last one without a newline so nothing scrolls, then wait.
function FillRows($t) {
    $script = Join-Path $data 'fill.ps1'
    Set-Content -Path $script -Encoding UTF8 -Value @'
$h = [Console]::WindowHeight
[Console]::Write("`e[2J`e[H")
for ($r = 1; $r -le $h; $r++) {
    [Console]::Write("`e[$r;1HROW$r")
}
[Console]::Write("`e[1;10H")
Start-Sleep -Seconds 30
'@
    P send-keys -t $t "clear; & '$script'" Enter | Out-Null
    $lines = @()
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        $lines = @(P capture-pane -t $t -p)
        if (($lines -join "`n") -match 'ROW1\b' -and ($lines -join "`n") -match 'ROW\d+\s*$') { break }
    }
    P send-keys -t $t C-c | Out-Null
    return ,$lines
}

$procs = @()
$defBefore = $null
try {
    $env:PSMUX_DATA_DIR = $null
    $defBefore = (& $exe ls 2>&1 | Out-String)
    $env:PSMUX_DATA_DIR = $data

    Write-Host "`n=== Issue #883: status off gives its row back ===" -ForegroundColor Cyan

    # ---- Attached client in a real visible window ----
    $p = Start-Process -FilePath $exe -ArgumentList '-L', $ns, 'new-session', '-s', 'a' -PassThru
    $procs += $p
    if (-not (WaitSession 'a') -or -not (WaitAttached 'a')) { Check 'attached session starts' $false ''; throw 'no session' }
    Start-Sleep -Seconds 2

    Write-Host "`n[1] status on" -ForegroundColor Yellow
    $hOn = PaneH 'a'
    $T = $hOn + 1
    Check 'pane is T-1 rows with the status on' ($hOn -gt 5) "pane_height=$hOn T=$T"
    Check 'client_height is T with the status on' ((Fmt 'a' '#{client_height}') -eq "$T") "client_height=$(Fmt 'a' '#{client_height}')"

    Write-Host "`n[2] set -g status off" -ForegroundColor Yellow
    P set -g status off | Out-Null
    $v = WaitFor 'a' '#{pane_height}' $T
    Check 'pane_height is T with the status off' ($v -eq "$T") "pane_height=$v want $T"
    Check 'window_height is T with the status off' ((Fmt 'a' '#{window_height}') -eq "$T") "window_height=$(Fmt 'a' '#{window_height}')"
    Check 'client_height stays T' ((Fmt 'a' '#{client_height}') -eq "$T") "client_height=$(Fmt 'a' '#{client_height}')"
    $inner = InnerRows 'a'
    Check 'ConPTY was resized: the shell sees T rows' ($inner -eq $T) "[Console]::WindowHeight=$inner want $T"

    Write-Host "`n[3] fullscreen app draws on the last row" -ForegroundColor Yellow
    $cap = FillRows 'a'
    $last = ($cap | Where-Object { $_ -match 'ROW\d+' } | Select-Object -Last 1)
    Check 'capture has T rows' ($cap.Count -eq $T) "rows=$($cap.Count) want $T"
    Check "last marker is ROW$T on the last row" (("$($cap[-1])".Trim()) -eq "ROW$T") "last row='$("$($cap[-1])".Trim())' last marker='$last'"

    Write-Host "`n[4] status 2, status 3, back to off, back on" -ForegroundColor Yellow
    P set -g status 2 | Out-Null
    $v = WaitFor 'a' '#{pane_height}' ($T - 2)
    Check 'status 2: pane is T-2' ($v -eq "$($T - 2)") "pane_height=$v"
    Check 'status 2: #{status} reads 2' ((Fmt 'a' '#{status}') -eq '2') "status=$(Fmt 'a' '#{status}')"
    Check 'status 2: client_height is T' ((Fmt 'a' '#{client_height}') -eq "$T") "client_height=$(Fmt 'a' '#{client_height}')"
    P set -g status 3 | Out-Null
    $v = WaitFor 'a' '#{pane_height}' ($T - 3)
    Check 'status 3: pane is T-3' ($v -eq "$($T - 3)") "pane_height=$v"
    P set -g status off | Out-Null
    $v = WaitFor 'a' '#{pane_height}' $T
    Check 'status 3 then off: pane is T' ($v -eq "$T") "pane_height=$v"
    P set -g status on | Out-Null
    $v = WaitFor 'a' '#{pane_height}' ($T - 1)
    Check 'back on: pane is T-1' ($v -eq "$($T - 1)") "pane_height=$v"

    Write-Host "`n[5] per session status" -ForegroundColor Yellow
    P set status off | Out-Null
    $v = WaitFor 'a' '#{pane_height}' $T
    Check 'set status off (session): pane is T' ($v -eq "$T") "pane_height=$v"
    P set -u status | Out-Null
    $v = WaitFor 'a' '#{pane_height}' ($T - 1)
    Check 'set -u status: pane is T-1 again' ($v -eq "$($T - 1)") "pane_height=$v"

    Write-Host "`n[6] two panes and zoom with the status off" -ForegroundColor Yellow
    P set -g status off | Out-Null
    $null = WaitFor 'a' '#{pane_height}' $T
    P split-window -v -t 'a' | Out-Null
    Start-Sleep -Milliseconds 800
    $hs = @(P list-panes -t 'a' -F '#{pane_height}') | ForEach-Object { [int]$_ }
    $sum = ($hs | Measure-Object -Sum).Sum + 1
    Check 'two panes plus the divider fill T rows' ($sum -eq $T) "heights=$($hs -join ',') +1 = $sum want $T"
    P resize-pane -Z -t 'a' | Out-Null
    $v = WaitFor 'a' '#{pane_height}' $T
    Check 'zoomed pane is T rows' ($v -eq "$T") "pane_height=$v zoomed=$(Fmt 'a' '#{window_zoomed_flag}')"
    P resize-pane -Z -t 'a' | Out-Null

    # ---- Config file at startup ----
    Write-Host "`n[7] set -g status off in the config at startup" -ForegroundColor Yellow
    $conf = Join-Path $data 'off.conf'
    Set-Content -Path $conf -Value 'set -g status off' -Encoding UTF8
    $env:PSMUX_CONFIG_FILE = $conf
    $p2 = Start-Process -FilePath $exe -ArgumentList '-L', $ns, 'new-session', '-s', 'c' -PassThru
    $procs += $p2
    $env:PSMUX_CONFIG_FILE = $emptyConf
    if ((WaitSession 'c') -and (WaitAttached 'c')) {
        Start-Sleep -Seconds 2
        Check 'config: status is off' ((Fmt 'c' '#{status}') -eq 'off') "status=$(Fmt 'c' '#{status}')"
        $v = WaitFor 'c' '#{pane_height}' $T
        Check 'config: pane is T rows from the start' ($v -eq "$T") "pane_height=$v want $T"
        $inner = InnerRows 'c'
        Check 'config: the shell sees T rows' ($inner -eq $T) "[Console]::WindowHeight=$inner want $T"
    } else {
        Check 'config session starts attached' $false ''
    }

    # ---- Detached ----
    Write-Host "`n[8] detached session keeps its size" -ForegroundColor Yellow
    P new-session -d -s d -x 100 -y 40 | Out-Null
    $null = WaitSession 'd'
    Start-Sleep -Milliseconds 800
    $d1 = PaneH 'd'
    P set -t d status off | Out-Null
    Start-Sleep -Milliseconds 600
    $d2 = PaneH 'd'
    Check 'detached: -y 40 stays 40 rows with the status on and off' ($d1 -eq 40 -and $d2 -eq 40) "on=$d1 off=$d2"
}
finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    foreach ($p in $procs) { try { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } } catch {} }
    $env:PSMUX_DATA_DIR = $null
    if ($null -ne $defBefore) {
        $defAfter = (& $exe ls 2>&1 | Out-String)
        if ($defAfter -ne $defBefore) { Write-Host "  [FAIL] default namespace sessions changed during the test" -ForegroundColor Red; $script:fail++ }
    }
    $env:PSMUX_DATA_DIR = $oldData
    $env:PSMUX_CONFIG_FILE = $oldConf
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:pass) passed, $($script:fail) failed ===" -ForegroundColor $(if ($script:fail) { 'Red' } else { 'Green' })
exit $script:fail
