# Issue #700: "set -ga terminal-overrides ',*:smcup@:rmcup@' keeps the client
# on the host terminal's main screen in tmux. psmux always sent ESC[?1049h."
#
# This hosts a real attached client inside a CreatePseudoConsole
# (tests/conpty697.cs), exactly the way Windows Terminal or an SSH session
# hosts psmux, records every byte the client writes to its host, and counts
# ESC[?1049h / ESC[?1049l.
#
# Measured on master 39ac085 (3 runs each, TERM=xterm-256color):
#     no override          1049h=1 1049l=1
#     ,*:smcup@:rmcup@     1049h=1 1049l=1   <- the bug
# Real tmux 3.4 in WSL with the same config: 1049h=1/1049l=1 without, 0/0 with.
# After the fix psmux matches tmux: 1/1 without, 0/0 with.
#
# Also covered: TERM unset (matched as the empty string, so `*` applies),
# a pattern that does not match TERM, runtime `set -ga` for a NEW attach,
# `set -g` replacing the array, and show-options printing the array like tmux.
#
# The parser, fnmatch and byte policy are covered with no server by
# tests-rs/test_issue700_terminal_overrides.rs.

$ErrorActionPreference = "Continue"

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }
Write-Host "  [INFO] binary under test: $PSMUX"

$SOCK = "i700_$PID"
$work = Join-Path $env:TEMP "psmux_i700"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$savedTerm = $env:TERM
function Finish {
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    if ($null -eq $savedTerm) { Remove-Item Env:TERM -EA SilentlyContinue } else { $env:TERM = $savedTerm }
    Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ==="
    if ($script:TestsFailed -gt 0) { exit 1 }
    exit 0
}

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { Write-Skip "csc.exe not found, cannot build the pseudoconsole harness"; Finish }
$harness = Join-Path $work "conpty697.exe"
& $csc /nologo /optimize /out:$harness (Join-Path $PSScriptRoot "conpty697.cs") 2>&1 | Out-Null
if (-not (Test-Path $harness)) { Write-Skip "the pseudoconsole harness did not compile"; Finish }

$confOn = Join-Path $work "on.conf"
$confOff = Join-Path $work "off.conf"
$confOther = Join-Path $work "other.conf"
"set -ga terminal-overrides ',*:smcup@:rmcup@'" | Set-Content $confOn -Encoding ASCII
"" | Set-Content $confOff -Encoding ASCII
"set -ga terminal-overrides ',screen*:smcup@:rmcup@'" | Set-Content $confOther -Encoding ASCII

# Run a client in the pseudoconsole for $waitMs and count the alt screen
# switches it wrote to its host.
function Invoke-Client([string]$name, [string]$clientArgs, [int]$waitMs) {
    $out = Join-Path $work "$name.bin"
    $scr = Join-Path $work "$name.txt"
    Remove-Item $out, "$out.idx" -Force -EA SilentlyContinue
    @("WAIT $waitMs", "END") | Set-Content $scr -Encoding ASCII
    & $harness $scr $out 100 30 0 "`"$PSMUX`" $clientArgs"
    if (-not (Test-Path $out)) { return $null }
    $t = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($out))
    return [pscustomobject]@{
        H = ([regex]::Matches($t, "`e\[\?1049h")).Count
        L = ([regex]::Matches($t, "`e\[\?1049l")).Count
        Status = $t -match '\[c\d\] 0:'
        Len = $t.Length
    }
}

# A session whose shell exits after 3 s, so the client exits on its own and
# its exit sequence is captured too.
function Invoke-NewSession([string]$name, [string]$conf) {
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    $r = Invoke-Client $name "-L $SOCK -f `"$conf`" new-session -s c1 `"pwsh -NoProfile -Command Start-Sleep 3`"" 9000
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    return $r
}

$env:TERM = "xterm-256color"

Write-Test "baseline: no override enters and leaves the alternate screen once"
$r = Invoke-NewSession "off" $confOff
if (-not $r) { Write-Fail "no capture"; Finish }
if (-not $r.Status) { Write-Fail "the client never drew its status line ($($r.Len) bytes), the capture is not a real attach"; Finish }
if ($r.H -eq 1 -and $r.L -eq 1) { Write-Pass "1049h=$($r.H) 1049l=$($r.L)" }
else { Write-Fail "expected 1049h=1 1049l=1, got 1049h=$($r.H) 1049l=$($r.L)" }

Write-Test "config: set -ga terminal-overrides ',*:smcup@:rmcup@' (TERM=xterm-256color)"
for ($i = 1; $i -le 3; $i++) {
    $r = Invoke-NewSession "on_$i" $confOn
    if ($r -and $r.Status -and $r.H -eq 0 -and $r.L -eq 0) { Write-Pass "run $i : 1049h=0 1049l=0, status line drawn on the main screen" }
    else { Write-Fail "run $i : expected 1049h=0 1049l=0, got 1049h=$($r.H) 1049l=$($r.L) status=$($r.Status) (#700)" }
}

Write-Test "config: TERM unset is matched as the empty string, * still applies"
Remove-Item Env:TERM -EA SilentlyContinue
$r = Invoke-NewSession "on_noterm" $confOn
if ($r -and $r.Status -and $r.H -eq 0 -and $r.L -eq 0) { Write-Pass "1049h=0 1049l=0 with no TERM" }
else { Write-Fail "expected 1049h=0 1049l=0 with no TERM, got 1049h=$($r.H) 1049l=$($r.L)" }
$env:TERM = "xterm-256color"

Write-Test "config: a pattern that does not match TERM changes nothing"
$r = Invoke-NewSession "other" $confOther
if ($r -and $r.H -eq 1 -and $r.L -eq 1) { Write-Pass "screen* does not match xterm-256color: 1049h=1 1049l=1" }
else { Write-Fail "expected 1049h=1 1049l=1, got 1049h=$($r.H) 1049l=$($r.L)" }

Write-Test "runtime: set -ga applies to a NEW attach, set -g replaces the array"
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
& $PSMUX -L $SOCK -f $confOff new-session -d -s c1 "pwsh -NoProfile -Command Start-Sleep 120"
Start-Sleep -Milliseconds 800
& $PSMUX -L $SOCK set -ga terminal-overrides ',*:smcup@:rmcup@'
$r = Invoke-Client "rt_ga" "-L $SOCK attach -t c1" 3500
if ($r -and $r.Status -and $r.H -eq 0) { Write-Pass "after runtime set -ga the attach sent 1049h=0" }
else { Write-Fail "after runtime set -ga expected 1049h=0, got 1049h=$($r.H) status=$($r.Status)" }

$show = (& $PSMUX -L $SOCK show-options -g terminal-overrides 2>&1) -join "`n"
$show = $show -replace "`n", "|"
if ($show -eq "terminal-overrides[0] linux*:AX@|terminal-overrides[1] *:smcup@:rmcup@") { Write-Pass "show-options -g prints tmux's default and the new element: $show" }
else { Write-Fail "show-options -g printed '$show', expected 'terminal-overrides[0] linux*:AX@|terminal-overrides[1] *:smcup@:rmcup@'" }

& $PSMUX -L $SOCK set -ga terminal-overrides 'xterm*:Tc'
$show = (& $PSMUX -L $SOCK show-options -g terminal-overrides 2>&1) -join "|"
if ($show -eq "terminal-overrides[0] linux*:AX@|terminal-overrides[1] *:smcup@:rmcup@|terminal-overrides[2] xterm*:Tc") { Write-Pass "set -ga without a leading comma appends element [2]: $show" }
else { Write-Fail "after a second set -ga, show printed '$show'" }
$showv = (& $PSMUX -L $SOCK show-options -gv terminal-overrides 2>&1) -join "|"
if ($showv -eq "linux*:AX@|*:smcup@:rmcup@|xterm*:Tc") { Write-Pass "show-options -gv prints the values one per line" }
else { Write-Fail "show-options -gv printed '$showv'" }

& $PSMUX -L $SOCK set -g terminal-overrides 'screen*:smcup@:rmcup@'
$show = (& $PSMUX -L $SOCK show-options -g terminal-overrides 2>&1) -join "|"
if ($show -eq "terminal-overrides[0] screen*:smcup@:rmcup@") { Write-Pass "set -g replaced the array: $show" }
else { Write-Fail "after set -g, show printed '$show'" }
$r = Invoke-Client "rt_g" "-L $SOCK attach -t c1" 3500
if ($r -and $r.H -eq 1) { Write-Pass "after set -g to a non matching pattern the attach sent 1049h=1 again" }
else { Write-Fail "after set -g expected 1049h=1, got 1049h=$($r.H)" }

& $PSMUX -L $SOCK set -gu terminal-overrides
$show = (& $PSMUX -L $SOCK show-options -g terminal-overrides 2>&1) -join "|"
if ($show -eq "terminal-overrides[0] linux*:AX@") { Write-Pass "set -gu puts tmux's default back: $show" }
else { Write-Fail "after set -gu, show printed '$show'" }

Finish
