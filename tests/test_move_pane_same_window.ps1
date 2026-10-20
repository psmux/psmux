# move-pane inside one window. tmux: move-pane is "like join-pane but allows
# the same window" (CHANGES 1.7), and cmd-join-pane.c refuses only
# `src_wp == dst_wp` ("source and target panes must be different"). psmux
# answered "can't join a pane to its own window" for every move-pane whose
# panes shared a window, from the CLI and from a control client (iTerm2's
# "Move Session to Split Pane" sends `move-pane -s %A -t %B -h` in one tab).
# join-pane keeps its refusal.
#
# Private -L namespace, cleaned up with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($cond, $msg) { if ($cond) { Write-Pass $msg } else { Write-Fail $msg } }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$ns = 'mpsw_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
function P { & $PSMUX -L $ns @args 2>&1 }
function Panes([string]$t) { @(P list-panes -t $t -F '#{pane_id}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) }
function Wait-Panes([string]$t, [int]$n) {
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) { if ((Panes $t).Count -eq $n) { return $true }; Start-Sleep -Milliseconds 200 }
    return $false
}
function Three-Panes([string]$s) {
    P kill-session -t $s | Out-Null
    P new-session -d -s $s -x 120 -y 40 | Out-Null
    [void](Wait-Panes $s 1)
    P split-window -h -t $s | Out-Null
    [void](Wait-Panes $s 2)
    P split-window -v -t $s | Out-Null
    [void](Wait-Panes $s 3)
    return , (Panes $s)
}

try {
    Write-Host "`n[1] CLI move-pane inside the window" -ForegroundColor Yellow
    $p = Three-Panes 'cli'
    $out = P move-pane -s $p[2] -t $p[0] -h
    $rc = $LASTEXITCODE
    Check ($rc -eq 0) "move-pane -s $($p[2]) -t $($p[0]) -h exits 0 (rc=$rc out=<$($out -join ' | ')>)"
    $after = Panes 'cli'
    Check (($after -join ',') -eq (@($p[0], $p[2], $p[1]) -join ',')) "the moved pane follows its target: $($after -join ',')"
    Check (@(P list-windows -t cli).Count -eq 1) "no window is made or closed"

    Write-Host "`n[2] CLI move-pane onto itself and join-pane inside the window are refused" -ForegroundColor Yellow
    $out = P move-pane -s $p[1] -t $p[1]
    Check ($LASTEXITCODE -ne 0 -and ($out -join ' ') -match 'must be different') "move-pane onto itself: $($out -join ' ')"
    $out = P join-pane -s 'cli:0.2' -t 'cli:0.0'
    Check ($LASTEXITCODE -ne 0 -and ($out -join ' ') -match 'own window') "join-pane in one window keeps its refusal: $($out -join ' ')"

    Write-Host "`n[3] control client move-pane inside the window" -ForegroundColor Yellow
    $p = Three-Panes 'cc'
    $psi = [Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $ns -C attach-session -t cc")
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.UseShellExecute = $false
    $cc = [Diagnostics.Process]::Start($psi)
    $read = $cc.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 1500
    $cc.StandardInput.WriteLine("move-pane -s `"$($p[2])`" -t `"$($p[0])`" -h -b"); $cc.StandardInput.Flush()
    Start-Sleep -Milliseconds 1500
    $cc.StandardInput.Close()
    if (-not $cc.WaitForExit(5000)) { $cc.Kill() }
    $lines = $read.Result -split "`r?`n"
    $err = @($lines | Where-Object { $_ -match '^%error ' })
    Check ($err.Count -eq 0) "no %error for the move ($(($lines | Where-Object { $_ -notmatch '^%(output|begin|end)' }) -join ' / '))"
    Check (@($lines | Where-Object { $_ -match '^%layout-change ' }).Count -ge 1) "a %layout-change follows the move"
    $after = Panes 'cc'
    Check (($after -join ',') -eq (@($p[2], $p[0], $p[1]) -join ',')) "-b puts the moved pane ahead of its target: $($after -join ',')"
} finally {
    & $PSMUX -L $ns kill-server 2>$null
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
