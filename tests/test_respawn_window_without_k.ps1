# respawn-window without -k on a window whose panes are still running.
#
# tmux 3.8 (spawn.c spawn_window, SPAWN_RESPAWN without SPAWN_KILL) refuses
# while ANY pane of the window has a live process:
#   respawn window failed: window <session>:<index> still active   (exit 1)
# and respawns a window whose panes have all exited (remain-on-exit). psmux
# killed the active pane's process with or without -k, so a bare
# `respawn-window` in a script or a binding threw away a running program.
#
# Every entry point is checked: the CLI and its alias, a source-file line, a
# key binding of an attached client (status line), control mode (%error),
# and the shared server mode. respawn-pane's own refusal is unchanged.
#
# Second part, the respawn itself (spawn.c, -k or every pane dead): tmux keeps
# only the window's FIRST pane, with its pane id, respawns it at the full
# window size, unzooms the window, and fires no pane-exited / pane-died hook
# for the panes it destroys. psmux respawned the active pane and kept the rest.
#
# Isolation: a unique -L namespace, a private PSMUX_DATA_DIR, an empty config;
# cleanup only with `-L <ns> kill-server` and the client windows by PID.
# Pass -Exe or set PSMUX_TEST_EXE / PSMUX_TEST_BIN.
param([string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }))

$ErrorActionPreference = "Continue"
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $m, $detail) { if ($ok) { Write-Pass $m } else { Write-Fail "$m ($detail)" } }

$injector = Join-Path $env:TEMP "psmux_respawnw_injector.exe"
if (-not (Test-Path $injector)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize "/out:$injector" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}

$saved = @{ PSMUX_DATA_DIR = $env:PSMUX_DATA_DIR; PSMUX_SERVER_MODE = $env:PSMUX_SERVER_MODE; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE; PSMUX_RUN_NOPAUSE = $env:PSMUX_RUN_NOPAUSE }
foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME') { Remove-Item "Env:\$v" -EA SilentlyContinue }
Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
Remove-Item Env:\PSMUX_SERVER_MODE -EA SilentlyContinue
$defaultBefore = (& $Exe ls 2>&1 | Out-String).Trim()

$tag = Get-Random
$ns = "rspw$tag"
$data = Join-Path $env:TEMP "psmux_$ns"
New-Item -ItemType Directory $data -Force | Out-Null
$env:PSMUX_DATA_DIR = $data
$env:PSMUX_RUN_NOPAUSE = "1"
$empty = Join-Path $data "empty.conf"
"" | Set-Content $empty
$env:PSMUX_CONFIG_FILE = $empty
$script:Ns = $ns
function P { & $Exe -L $script:Ns @args 2>&1 }
function Pids($t) { ((P list-panes -t $t -F '#{pane_pid}') -join ',') }
function WaitFor([scriptblock]$cond, [int]$ms = 6000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
function Status($s) {
    $dump = (P dump-state -t $s) -join ''
    if ($dump -match '"status_message":"([^"]*)"') { $Matches[1] } else { '' }
}

$proc = $null; $cc = $null
Write-Host "`n=== respawn-window without -k (tmux spawn.c) ===" -ForegroundColor Cyan
try {
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1200
    P split-window -d -t s:0 | Out-Null
    Start-Sleep -Milliseconds 800
    $p0 = Pids s:0

    # ---- CLI ----
    $o = (P respawn-window -t s:0) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 1) "respawn-window on a live window exits 1" "rc=$rc"
    Check ($o -match 'respawn window failed: window s:0 still active') "with tmux's words" $o
    Start-Sleep -Milliseconds 500
    Check ((Pids s:0) -eq $p0) "and no pane got a new process" "$p0 -> $(Pids s:0)"
    $o = (P respawnw -t s:0) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 1 -and $o -match 'still active') "the respawnw alias is refused too" "rc=$rc $o"
    $o = (P respawn-window -t s:0 -c $env:TEMP 'cmd /k echo NOPE') -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 1 -and (Pids s:0) -eq $p0) "with -c and a command operand too" "rc=$rc $o"

    # ---- source-file ----
    $conf = Join-Path $data "rw.conf"
    "respawn-window -t s:0" | Set-Content $conf
    P source-file $conf | Out-Null
    Start-Sleep -Milliseconds 800
    Check ((Pids s:0) -eq $p0) "a source-file line kills nothing" "$p0 -> $(Pids s:0)"

    # ---- -k, as tmux spawn.c: only the FIRST pane is kept (with its id),
    # respawned at the full window size, the window unzoomed, and the
    # panes that go fire no pane-exited / pane-died hook ----
    P split-window -d -h -t s:0.1 | Out-Null
    Start-Sleep -Milliseconds 800
    P select-pane -t s:0.2 | Out-Null
    P resize-pane -Z -t s:0.2 | Out-Null
    $ids0 = @((P list-panes -t s:0 -F '#{pane_id}'))
    $pids0 = @((P list-panes -t s:0 -F '#{pane_pid}'))
    $zoom0 = (P display -p -t s:0 '#{window_zoomed_flag}') -join ''
    P set-hook -g pane-exited 'set -g @rw_hook exited' | Out-Null
    P set-hook -g pane-died 'set -g @rw_hook died' | Out-Null
    $o = (P respawn-window -k -t s:0) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 0) "respawn-window -k exits 0 (3 panes, pane 2 active and zoomed=$zoom0)" "rc=$rc $o"
    Start-Sleep -Milliseconds 1200
    $ids1 = @((P list-panes -t s:0 -F '#{pane_id}'))
    Check ($ids1.Count -eq 1 -and $ids1[0] -eq $ids0[0]) "only the first pane is left, with its id ($($ids0 -join ',') -> $($ids1 -join ','))" ""
    $a = (P display -p -t s:0 '#{pane_id} #{pane_active} #{window_zoomed_flag} #{pane_width}x#{pane_height} #{window_width}x#{window_height}') -join ''
    $f = $a -split ' '
    Check ($f[0] -eq $ids0[0] -and $f[1] -eq '1') "the first pane is active" $a
    Check ($f[2] -eq '0') "the window is not zoomed" $a
    Check ($f[3] -eq $f[4]) "the pane fills the window" $a
    $newPid = (P display -p -t $ids0[0] '#{pane_pid}') -join ''
    Check ($newPid -and $newPid -ne $pids0[0]) "the first pane runs a new process ($($pids0[0]) -> $newPid)" ""
    $gone = @($pids0 | Select-Object -Skip 1 | Where-Object { Get-Process -Id $_ -EA SilentlyContinue })
    Check (WaitFor { @($pids0 | Select-Object -Skip 1 | Where-Object { Get-Process -Id $_ -EA SilentlyContinue }).Count -eq 0 }) "the other panes' processes ended" ($gone -join ',')
    $hk = (P show -gv '@rw_hook') -join ''
    Check ($hk -notmatch 'exited|died') "no pane-exited / pane-died hook fired for them, as in tmux" "@rw_hook=$hk"
    $p0 = Pids s:0

    # ---- respawn-pane is unchanged ----
    $o = (P respawn-pane -t s:0.0) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 1 -and $o -match 'pane s:0.0 still active') "respawn-pane without -k keeps its own refusal" "rc=$rc $o"

    # ---- dead panes (remain-on-exit) ----
    P set -g remain-on-exit on | Out-Null
    P new-window -d -t s:3 -n dead 'cmd /c exit 0' | Out-Null
    Check (WaitFor { ((P display -p -t s:dead '#{pane_dead}') -join '') -eq '1' }) "a window whose only pane exited is dead" ""
    # positive control for the hook check above: a pane whose process exits
    # under remain-on-exit does fire the hooks (pane-died, then pane-exited)
    Check (WaitFor { ((P show -gv '@rw_hook') -join '') -match '^(died|exited)$' }) "(control) the hooks fire for a pane whose process exits" ((P show -gv '@rw_hook') -join '')
    P set-hook -gu pane-exited | Out-Null
    P set-hook -gu pane-died | Out-Null
    $o = (P respawn-window -t s:dead 'cmd /k echo RESPAWNED_OK') -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 0) "respawn-window without -k respawns a dead window" "rc=$rc $o"
    Check (WaitFor { ((P display -p -t s:dead '#{pane_dead}') -join '') -eq '0' }) "and its pane runs again" ""
    Check (WaitFor { ((P capture-pane -p -t s:dead) -join "`n") -match 'RESPAWNED_OK' }) "with the new command" ""

    P new-window -d -t s:4 -n half 'cmd /c exit 0' | Out-Null
    Start-Sleep -Milliseconds 1200
    P split-window -d -t s:half | Out-Null
    Start-Sleep -Milliseconds 800
    $flags = ((P list-panes -t s:half -F '#{pane_dead}') -join ',')
    $o = (P respawn-window -t s:half) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 1 -and $o -match 'window s:4 still active') "one dead pane and one live pane: still refused (panes dead=$flags)" "rc=$rc $o"

    # every pane dead: no -k needed, and again only the first pane is kept
    P new-window -d -t s:6 -n alldead 'cmd /c exit 0' | Out-Null
    Start-Sleep -Milliseconds 1000
    P split-window -d -t s:alldead 'cmd /c exit 0' | Out-Null
    Check (WaitFor { ((P list-panes -t s:alldead -F '#{pane_dead}') -join ',') -eq '1,1' }) "a window with two dead panes" ((P list-panes -t s:alldead -F '#{pane_dead}') -join ',')
    $ad0 = @((P list-panes -t s:alldead -F '#{pane_id}'))
    $o = (P respawn-window -t s:alldead 'cmd /k echo ALLDEAD_BACK') -join ' '; $rc = $LASTEXITCODE
    Start-Sleep -Milliseconds 1000
    $ad1 = @((P list-panes -t s:alldead -F '#{pane_id}:#{pane_dead}'))
    Check ($rc -eq 0 -and $ad1.Count -eq 1 -and $ad1[0] -eq "$($ad0[0]):0") "respawn-window keeps the first pane and runs it ($($ad0 -join ',') -> $($ad1 -join ','))" "rc=$rc $o"
    P set -g remain-on-exit off | Out-Null

    # ---- a key binding of an attached client: the status line says why ----
    P set -g display-time 8000 | Out-Null
    $proc = Start-Process -FilePath $Exe -ArgumentList "-L", $ns, "attach", "-t", "s" -PassThru
    Check (WaitFor { ((P display -p -t s '#{session_attached}') -join '') -eq '1' } 8000) "a client attaches" ""
    P select-window -t s:0 | Out-Null
    P bind -n F9 respawn-window | Out-Null
    Start-Sleep -Milliseconds 600
    $p0 = Pids s:0
    & $injector $proc.Id "{F9}" | Out-Null
    Start-Sleep -Milliseconds 1200
    $sm = Status s
    Check ($sm -match 'window s:0 still active') "a binding's respawn-window is refused on the status line" "'$sm'"
    Check ((Pids s:0) -eq $p0) "and kills nothing" "$p0 -> $(Pids s:0)"
    P bind -n F9 respawn-window -k | Out-Null
    Start-Sleep -Milliseconds 400
    $a0 = ((P display -p -t s:0 '#{pane_pid}') -join '')
    & $injector $proc.Id "{F9}" | Out-Null
    Check (WaitFor { ((P display -p -t s:0 '#{pane_pid}') -join '') -ne $a0 }) "a binding's respawn-window -k still respawns" ""
    P unbind -n F9 | Out-Null

    # ---- control mode: %error ----
    $psi = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in @('-L', $ns, '-C', 'attach', '-t', 's')) { $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.UseShellExecute = $false
    $cc = [Diagnostics.Process]::Start($psi)
    $outTask = $cc.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 800
    $p0 = Pids s:0
    $cc.StandardInput.WriteLine('respawn-window -t s:0')
    $cc.StandardInput.Flush()
    Start-Sleep -Milliseconds 1200
    $cc.StandardInput.WriteLine('detach-client')
    $cc.StandardInput.Close()
    [void]$cc.WaitForExit(5000)
    $ctl = if ($outTask.Wait(3000)) { $outTask.Result } else { '' }
    Check ($ctl -match '%error' -and $ctl -match 'still active') "control mode answers %error" (($ctl -split "`n" | Select-String 'error|still' | Select-Object -First 3) -join ' / ')
    Check ((Pids s:0) -eq $p0) "and kills nothing" "$p0 -> $(Pids s:0)"
} finally {
    if ($cc -and -not $cc.HasExited) { Stop-Process -Id $cc.Id -Force -EA SilentlyContinue }
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    P kill-server | Out-Null
}

# ---- shared server mode (behind the switch) ----
$script:Ns = "rspwS$tag"
$env:PSMUX_SERVER_MODE = "shared"
try {
    P new-session -d -s a -x 120 -y 30 | Out-Null
    P new-session -d -s b -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1200
    $pa = (P display -p -t a '#{pid}') -join ''; $pb = (P display -p -t b '#{pid}') -join ''
    Check ($pa -and $pa -eq $pb) "shared: a and b share one server" "a=$pa b=$pb"
    $p0 = Pids b:0
    $o = (P respawn-window -t b:0) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 1 -and $o -match 'window b:0 still active') "shared: respawn-window without -k is refused" "rc=$rc $o"
    Check ((Pids b:0) -eq $p0) "shared: and kills nothing" ""
    $o = (P respawn-window -k -t b:0) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 0 -and (WaitFor { (Pids b:0) -ne $p0 })) "shared: -k respawns" "rc=$rc $o"
} finally {
    P kill-server | Out-Null
    foreach ($k in $saved.Keys) {
        if ($null -eq $saved[$k]) { Remove-Item "Env:\$k" -EA SilentlyContinue } else { Set-Item "Env:\$k" $saved[$k] }
    }
    Start-Sleep -Milliseconds 400
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
}

$keepData = $env:PSMUX_DATA_DIR; $keepMode = $env:PSMUX_SERVER_MODE
Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
Remove-Item Env:\PSMUX_SERVER_MODE -EA SilentlyContinue
$defaultAfter = (& $Exe ls 2>&1 | Out-String).Trim()
if ($keepData) { $env:PSMUX_DATA_DIR = $keepData }
if ($keepMode) { $env:PSMUX_SERVER_MODE = $keepMode }
if ($defaultAfter -eq $defaultBefore) { Write-Pass "the default namespace is unchanged" } else { Write-Fail "default namespace changed: '$defaultBefore' -> '$defaultAfter'" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Passed)"
Write-Host "  Failed: $($script:Failed)"
exit $script:Failed
