# Issue #868: a cross-session join-pane whose -t the destination cannot take
# must be refused BEFORE the source pane is extracted. On master the CLI's
# check accepted a floating pane (and, with pane-base-index 1, the pane index
# as #{pane_index} shows it), the source gave the pane up, the inject refused
# it, and the pane was in neither session; 30 s later its shell was ended.
#
# Checks, for pane-base-index 0 and 1:
#   - -t <floating pane by index> and -t <floating pane by %id> are refused
#     with tmux's "can't split a floating pane", the source pane stays in the
#     source session, its shell stays alive and its screen keeps a marker;
#   - -t <second tiled pane as #{pane_index} shows it> moves the pane (control);
#   - -t <pane past the last> is refused and nothing moves.
#
# Isolation: unique -L namespace, private PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server`. PSMUX_TEST_EXE picks the binary (default: psmux on
# PATH).

$ErrorActionPreference = 'Continue'
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $m, $detail) { if ($ok) { Pass $m } else { Fail "$m -> $detail" } }

$Psmux = $env:PSMUX_TEST_EXE
if (-not $Psmux) { $f = Get-Command psmux -ErrorAction SilentlyContinue; if ($f) { $Psmux = $f.Source } }
if (-not $Psmux) { Write-Host "psmux binary not found"; exit 2 }
$Psmux = (Resolve-Path $Psmux).Path

$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$RootBase = if ($env:PSMUX_TEST_ROOT) { $env:PSMUX_TEST_ROOT } else { [System.IO.Path]::GetTempPath() }
$Root = Join-Path $RootBase ("psmux_868_" + $Tag)
$Ns = "t868" + $Tag
$SavedData = $env:PSMUX_DATA_DIR
$DefaultBefore = (& $Psmux ls 2>&1 | Out-String)
$env:PSMUX_DATA_DIR = Join-Path $Root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
foreach ($n in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL') {
    Remove-Item "Env:$n" -ErrorAction SilentlyContinue
}

function Run { $a = $args
    $out = & $Psmux -L $Ns @a 2>&1
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = (($out | Out-String).Trim()) }
}
function Panes($s) { (& $Psmux -L $Ns list-panes -s -t $s -F '#{pane_id}' 2>&1) -join ' ' }
function WaitFor($sb, $ms = 8000) {
    $dl = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $dl) { if (& $sb) { return $true }; Start-Sleep -Milliseconds 150 }
    return $false
}

$Case = 0

function New-Pair($Base) {
    $script:Case++
    $x = "x$($script:Case)"; $y = "y$($script:Case)"
    foreach ($s in $x, $y) {
        & $Psmux -L $Ns new-session -d -s $s -x 120 -y 40 | Out-Null
    }
    foreach ($s in $x, $y) {
        if (-not (WaitFor { (& $Psmux -L $Ns has-session -t $s 2>$null); $LASTEXITCODE -eq 0 })) { throw "session $s did not start" }
        & $Psmux -L $Ns set -t $s -g pane-base-index $Base | Out-Null
    }
    & $Psmux -L $Ns split-window -t $x -d | Out-Null
    & $Psmux -L $Ns new-pane -t $x | Out-Null
    & $Psmux -L $Ns split-window -t $y -d | Out-Null
    WaitFor { (Panes $x).Split(' ').Count -eq 3 -and (Panes $y).Split(' ').Count -eq 2 } | Out-Null
    $yid = ((& $Psmux -L $Ns list-panes -t "${y}:0" -F '#{pane_id}') | Select-Object -Last 1).Trim()
    $ypid = [int](((& $Psmux -L $Ns list-panes -t "${y}:0" -F '#{pane_pid}') | Select-Object -Last 1).Trim())
    $fid = ((& $Psmux -L $Ns list-panes -t "${x}:0" -F '#{pane_id}|#{pane_floating_flag}') | Where-Object { $_ -match '\|1$' } | Select-Object -First 1)
    $fid = if ($fid) { $fid.Split('|')[0] } else { '' }
    $marker = "M868_$($script:Case)_$Tag"
    & $Psmux -L $Ns send-keys -t "${y}:0.$($Base + 1)" "echo $marker" Enter | Out-Null
    WaitFor { (& $Psmux -L $Ns capture-pane -p -t "${y}:0.$($Base + 1)" 2>$null | Out-String) -match $marker } | Out-Null
    [pscustomobject]@{ Base = $Base; X = $x; Y = $y; YId = $yid; YPid = $ypid; Float = $fid; Marker = $marker }
}

function Assert-Kept($p, $what) {
    Check ((Panes $p.Y) -match [regex]::Escape($p.YId)) "$what : $($p.YId) is still in the source session" "Y panes '$(Panes $p.Y)'"
    Check ((Panes $p.X).Split(' ').Count -eq 3) "$what : nothing new in the destination" "X panes '$(Panes $p.X)'"
    Check ([bool](Get-Process -Id $p.YPid -ErrorAction SilentlyContinue)) "$what : the source shell is alive" "pid $($p.YPid)"
    $cap = (& $Psmux -L $Ns capture-pane -p -t "$($p.Y):0.$($p.Base + 1)" 2>&1 | Out-String)
    Check ($cap -match $p.Marker) "$what : the source pane still shows its marker" "capture '$($cap.Trim())'"
}

try {
    foreach ($Base in 0, 1) {
        Write-Host "`n--- pane-base-index $Base ---" -ForegroundColor Yellow
        $fIdx = $Base + 2; $second = $Base + 1; $past = $Base + 5

        $p = New-Pair $Base
        Check ($p.Float -ne '') "setup: the destination has a floating pane" "no floating pane"
        $r = Run join-pane -s "$($p.Y):$($p.YId)" -t "$($p.X):0.$fIdx"
        Check ($r.Code -eq 1 -and $r.Text -match "can't split a floating pane") "join-pane -t <float by index $fIdx> is refused" "[$($r.Code)] $($r.Text)"
        Assert-Kept $p "float by index"
        $r = Run join-pane -s "$($p.Y):$($p.YId)" -t "$($p.X):$($p.Float)"
        Check ($r.Code -eq 1 -and $r.Text -match "can't split a floating pane") "join-pane -t <float by id $($p.Float)> is refused" "[$($r.Code)] $($r.Text)"
        Assert-Kept $p "float by id"
        $r = Run join-pane -s "$($p.Y):$($p.YId)" -t "$($p.X):0.$past"
        Check ($r.Code -eq 1 -and $r.Text -match "can't find pane: $past") "join-pane -t <pane $past past the last> is refused" "[$($r.Code)] $($r.Text)"
        Assert-Kept $p "missing pane"
        $r = Run join-pane -s "$($p.Y):$($p.YId)" -t "$($p.X):0.$second"
        Check ($r.Code -eq 0) "join-pane -t <second tiled pane $second> succeeds" "[$($r.Code)] $($r.Text)"
        Check (WaitFor { (Panes $p.X).Split(' ').Count -eq 4 }) "the joined pane is in the destination" "X panes '$(Panes $p.X)'"
        Check (-not ((Panes $p.Y) -match [regex]::Escape($p.YId))) "and gone from the source" "Y panes '$(Panes $p.Y)'"
        $cap = (& $Psmux -L $Ns capture-pane -p -t "$($p.X):0.$($Base + 2)" 2>&1 | Out-String)
        Check ($cap -match $p.Marker) "the joined pane shows the source marker after pane $second" "capture '$($cap.Trim())'"
    }
}
catch { Fail "setup or unexpected error: $_" }
finally {
    & $Psmux -L $Ns kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $SavedData
    $DefaultAfter = (& $Psmux ls 2>&1 | Out-String)
    Check ($DefaultAfter -ceq $DefaultBefore) "default namespace ls unchanged" "before '$DefaultBefore' after '$DefaultAfter'"
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0) { exit 1 }
exit 0
