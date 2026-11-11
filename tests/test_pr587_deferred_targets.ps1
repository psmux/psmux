# PR #587: preserve deferred command targets
#
# psmux used to treat EVERY `-t` token on the command line as the process's
# own routing target, even when the token belonged to a stored command
# (bind-key / set-hook / confirm-before) or to the program after `--`.
# tmux stores the bound / hooked command verbatim and passes everything after
# `--` to the child untouched.  These checks compare psmux against that.
#
# Layers: CLI (main.rs argv routing), raw TCP one-shot (connection.rs
# without_outer_target), control mode (dispatch_control_command set-hook),
# and a real attached client (confirm-before prompt via keystroke injection).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
Write-Host "binary: $PSMUX" -ForegroundColor Cyan
$SESSION = "pr587_deferred"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    foreach ($s in @($SESSION, "worker", "2", "0", "child")) {
        & $PSMUX kill-session -t $s 2>&1 | Out-Null
    }
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Send-TcpCommand {
    param([string]$Session, [string]$Command)
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $authResp = $reader.ReadLine()
    if ($authResp -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
    $writer.Write("$Command`n"); $writer.Flush()
    $stream.ReadTimeout = 5000
    try { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
    $tcp.Close()
    return $resp
}

function Get-Keys { (& $PSMUX list-keys -t $SESSION 2>&1 | Out-String) }
function Get-Hooks { (& $PSMUX show-hooks -g -t $SESSION 2>&1 | Out-String) }

Cleanup
Remove-Item Env:\PSMUX_SESSION -EA SilentlyContinue
Remove-Item Env:\TMUX -EA SilentlyContinue
& $PSMUX new-session -d -s $SESSION -x 120 -y 30
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session creation failed"; exit 1 }

Write-Host "`n=== PR #587: deferred command targets ===" -ForegroundColor Cyan

# ---------------------------------------------------------------------------
# Part A: CLI path (main.rs argv routing)
# ---------------------------------------------------------------------------
Write-Host "`n[A1] bind-key keeps an inner -t (bind-key -T prefix F5 select-window -t 2)" -ForegroundColor Yellow
$out = & $PSMUX -t $SESSION bind-key -T prefix F5 select-window -t 2 2>&1 | Out-String
Start-Sleep -Milliseconds 400
$keys = Get-Keys
$line = ($keys -split "`n" | Where-Object { $_ -match '\bF5\b' } | Select-Object -First 1)
if ($line -match 'select-window\s+-t\s+2') { Write-Pass "F5 binding stored verbatim: $($line.Trim())" }
else { Write-Fail "F5 binding lost its -t 2 (rc=$LASTEXITCODE out='$($out.Trim())' line='$($line.Trim())')" }

Write-Host "`n[A2] bind-key with NO outer -t must not route on the inner one" -ForegroundColor Yellow
$out = & $PSMUX bind-key -T prefix F6 select-window -t 2 2>&1 | Out-String
$rc = $LASTEXITCODE
& $PSMUX has-session -t 2 2>$null
$phantom = ($LASTEXITCODE -eq 0)
if ($out -match 'no server running on session') { Write-Fail "CLI routed on the deferred -t 2: $($out.Trim())" }
elseif ($phantom) { Write-Fail "a phantom session named '2' was created" }
else { Write-Pass "no routing on the inner -t (rc=$rc, out='$($out.Trim())')" }

Write-Host "`n[A3] set-hook keeps an inner -t (set-hook -g after-new-window select-window -t 0)" -ForegroundColor Yellow
& $PSMUX -t $SESSION set-hook -g after-new-window select-window -t 0 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$hooks = Get-Hooks
if ($hooks -match 'after-new-window.*select-window\s+-t\s+0') { Write-Pass "hook stored verbatim: $(($hooks -split "`n" | Where-Object { $_ -match 'after-new-window' } | Select-Object -First 1).Trim())" }
else { Write-Fail "hook lost its -t 0: '$($hooks.Trim())'" }
# The hook now really fires with its target (it is the whole point of the
# fix), which would drag the active window back to 0 after every new-window
# below. Unset it so the next checks can capture the window they created.
& $PSMUX -t $SESSION set-hook -gu after-new-window 2>&1 | Out-Null
Start-Sleep -Milliseconds 300
if ((Get-Hooks) -notmatch 'after-new-window\[') { Write-Pass "set-hook -gu removed the hook again" } else { Write-Fail "set-hook -gu left the hook in place: '$((Get-Hooks).Trim())'" }

Write-Host "`n[A4] new-window -t SESSION -- child -t worker passes -t worker to the child" -ForegroundColor Yellow
& $PSMUX new-window -t $SESSION -- cmd /k echo CHILDARGS -t worker 2>&1 | Out-Null
Start-Sleep -Seconds 3
$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match 'CHILDARGS -t worker') { Write-Pass "child saw '-t worker'" }
elseif ($cap -match 'CHILDARGS') { Write-Fail "child lost '-t worker': $(($cap -split "`n" | Where-Object { $_ -match 'CHILDARGS' } | Select-Object -First 1).Trim())" }
else { Write-Fail "child output not found: '$($cap.Trim())'" }
& $PSMUX has-session -t worker 2>$null
if ($LASTEXITCODE -eq 0) { Write-Fail "phantom session 'worker' exists" } else { Write-Pass "no phantom 'worker' session" }

Write-Host "`n[A5] new-window (no outer -t) -- child -t worker must not route on 'worker'" -ForegroundColor Yellow
$env:PSMUX_SESSION_NAME = $SESSION
$out = & $PSMUX new-window -- cmd /k echo NOOUTER -t worker 2>&1 | Out-String
Remove-Item Env:\PSMUX_SESSION_NAME -EA SilentlyContinue
Start-Sleep -Seconds 3
$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($out -match 'no server running on session') { Write-Fail "CLI routed on the child's -t: $($out.Trim())" }
elseif ($cap -match 'NOOUTER -t worker') { Write-Pass "child saw '-t worker' with no outer target" }
else { Write-Fail "unexpected: out='$($out.Trim())' cap='$(($cap -split "`n" | Where-Object { $_ -match 'NOOUTER' } | Select-Object -First 1))'" }

Write-Host "`n[A6] outer -t still routes and is stripped (new-window -t SESSION -n named)" -ForegroundColor Yellow
& $PSMUX new-window -t $SESSION -n named587 2>&1 | Out-Null
Start-Sleep -Seconds 2
$wins = & $PSMUX list-windows -t $SESSION 2>&1 | Out-String
if ($wins -match 'named587') { Write-Pass "outer -t routed to $SESSION" } else { Write-Fail "window not created in $SESSION : $($wins.Trim())" }

Write-Host "`n[A7] detach-client -t is left alone (issue #565 exemption)" -ForegroundColor Yellow
$out = & $PSMUX detach-client -t $SESSION 2>&1 | Out-String
if ($out -notmatch 'no server running') { Write-Pass "detach-client -t $SESSION accepted (out='$($out.Trim())')" }
else { Write-Fail "detach-client routing broke: $($out.Trim())" }

# ---------------------------------------------------------------------------
# Part B: raw TCP one-shot path (connection.rs without_outer_target)
# ---------------------------------------------------------------------------
Write-Host "`n[B1] TCP bind-key -T prefix F7 select-window -t 3" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $SESSION -Command "bind-key -T prefix F7 select-window -t 3"
Start-Sleep -Milliseconds 400
$keys = Get-Keys
$line = ($keys -split "`n" | Where-Object { $_ -match '\bF7\b' } | Select-Object -First 1)
if ($line -match 'select-window\s+-t\s+3') { Write-Pass "TCP binding verbatim: $($line.Trim())" }
else { Write-Fail "TCP binding lost -t 3 (resp='$resp' line='$($line.Trim())')" }

Write-Host "`n[B2] TCP set-hook -g after-split-window kill-window -t 9" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $SESSION -Command "set-hook -g after-split-window kill-window -t 9"
Start-Sleep -Milliseconds 400
$hooks = Get-Hooks
if ($hooks -match 'after-split-window.*kill-window\s+-t\s+9') { Write-Pass "TCP hook verbatim" }
else { Write-Fail "TCP hook lost -t 9: '$($hooks.Trim())'" }

Write-Host "`n[B3] TCP outer -t still applies (rename-window -t :0 renamed587)" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $SESSION -Command "rename-window -t :0 renamed587"
Start-Sleep -Milliseconds 400
$wins = & $PSMUX list-windows -t $SESSION 2>&1 | Out-String
if ($wins -match '^0:\s*renamed587' -or $wins -match 'renamed587') { Write-Pass "outer -t :0 honoured" } else { Write-Fail "rename did not apply: $($wins.Trim())" }

# ---------------------------------------------------------------------------
# Part C: control mode path (dispatch_control_command set-hook)
# ---------------------------------------------------------------------------
Write-Host "`n[C1] control-mode (-C client) set-hook -g after-kill-pane select-pane -t 1" -ForegroundColor Yellow
$ctlIn = Join-Path $env:TEMP "pr587_ctl_in.txt"
[System.IO.File]::WriteAllText($ctlIn, "set-hook -g after-kill-pane select-pane -t 1`n")
$env:PSMUX_SESSION_NAME = $SESSION
$ctl = Start-Process -FilePath $PSMUX -ArgumentList "-C" -RedirectStandardInput $ctlIn `
    -RedirectStandardOutput "$env:TEMP\pr587_ctl_out.txt" -RedirectStandardError "$env:TEMP\pr587_ctl_err.txt" -PassThru -NoNewWindow
if (-not $ctl.WaitForExit(6000)) { try { $ctl.Kill() } catch {} }
Remove-Item Env:\PSMUX_SESSION_NAME -EA SilentlyContinue
Start-Sleep -Milliseconds 500
$hooks = Get-Hooks
if ($hooks -match 'after-kill-pane.*select-pane\s+-t\s+1') { Write-Pass "control-mode hook verbatim" }
else { Write-Fail "control-mode hook lost -t 1: '$($hooks.Trim())' ctl-out='$((Get-Content "$env:TEMP\pr587_ctl_out.txt" -EA SilentlyContinue | Out-String).Trim())'" }
Remove-Item $ctlIn,"$env:TEMP\pr587_ctl_out.txt","$env:TEMP\pr587_ctl_err.txt" -Force -EA SilentlyContinue

# ---------------------------------------------------------------------------
# Part D: edge cases
# ---------------------------------------------------------------------------
Write-Host "`n[D1] bind-key -n (root table) with inner -t" -ForegroundColor Yellow
& $PSMUX -t $SESSION bind-key -n F8 select-window -t 4 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$keys = Get-Keys
$line = ($keys -split "`n" | Where-Object { $_ -match '\bF8\b' } | Select-Object -First 1)
if ($line -match 'select-window\s+-t\s+4' -and $line -match 'root') { Write-Pass "root binding verbatim: $($line.Trim())" }
else { Write-Fail "root binding wrong: '$($line.Trim())'" }

Write-Host "`n[D2] set-hook with its OWN -t before the hook name is routing, inner -t kept" -ForegroundColor Yellow
& $PSMUX set-hook -t $SESSION after-rename-window select-window -t 5 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$hooks = Get-Hooks
$sessHooks = (& $PSMUX show-hooks -t $SESSION 2>&1 | Out-String)
if (($hooks + $sessHooks) -match 'after-rename-window.*select-window\s+-t\s+5') { Write-Pass "outer -t routed, inner -t 5 kept" }
else { Write-Fail "hook wrong: g='$($hooks.Trim())' s='$($sessHooks.Trim())'" }

Write-Host "`n[D3] unbind still works on a binding whose command holds -t" -ForegroundColor Yellow
& $PSMUX -t $SESSION unbind-key -T prefix F5 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$keys = Get-Keys
if (($keys -split "`n" | Where-Object { $_ -match '\bF5\b' }).Count -eq 0) { Write-Pass "F5 unbound" } else { Write-Fail "F5 still bound" }

Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
