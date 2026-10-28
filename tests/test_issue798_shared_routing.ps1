# #798 step 3 item 1: in process routing between the sessions of one shared
# server. Each check is an entry point that runs from inside the server and
# names ANOTHER session of it:
#   * a hook's command whose -t names a sibling (it reaches the hook's own
#     listener, where the per session handler answered "can't find session")
#   * a one shot line naming a sibling sent to another session's listener
#   * switch-client -t s:w.p, -t %N and -t @N to a window or pane of a sibling,
#     one shot and from a key binding (refused on master)
#   * new-session from a key binding and from the command prompt, with and
#     without -d (refused on master)
#   * the session chooser (choose-tree -s), which must keep working
#   * the loop stays responsive (no wait on itself) after each one
#
# Isolation: a unique -L namespace, a private PSMUX_DATA_DIR, an empty config;
# cleanup only with `-L <ns> kill-server` and the client window by PID.
# Pass -Exe or set PSMUX_TEST_EXE.
param([string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }))

$ErrorActionPreference = "Continue"
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $m, $detail) { if ($ok) { Write-Pass $m } else { Write-Fail "$m ($detail)" } }

$injector = Join-Path $env:TEMP "psmux_shared_routing_injector.exe"
if (-not (Test-Path $injector)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize "/out:$injector" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}

$saved = @{ PSMUX_DATA_DIR = $env:PSMUX_DATA_DIR; PSMUX_SERVER_MODE = $env:PSMUX_SERVER_MODE; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE }
foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME') { Remove-Item "Env:\$v" -EA SilentlyContinue }
Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
$defaultBefore = (& $Exe ls 2>&1 | Out-String).Trim()

$ns = "shroute$(Get-Random)"
$data = Join-Path $env:TEMP "psmux_$ns"
New-Item -ItemType Directory $data -Force | Out-Null
$env:PSMUX_DATA_DIR = $data
$empty = Join-Path $data "empty.conf"
"" | Set-Content $empty
$env:PSMUX_CONFIG_FILE = $empty
$env:PSMUX_SERVER_MODE = "shared"
function P { & $Exe -L $ns @args 2>&1 }
function Wn($s) { ((P list-windows -t $s -F '#{window_name}') -join ',') }
function Att($s) { ((P display -p -t $s '#{session_attached}') -join '') }
function PingMs { $sw = [Diagnostics.Stopwatch]::StartNew(); P display -p -t A ping | Out-Null; $sw.ElapsedMilliseconds }
function Has($s) { P has-session -t $s | Out-Null; $LASTEXITCODE -eq 0 }
function WaitFor([scriptblock]$cond, [int]$ms = 4000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
function Raw($sess, $line) {
    $port = (Get-Content (Join-Path $data "${ns}__$sess.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $data "${ns}__$sess.key") -Raw).Trim()
    $c = [Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $st = $c.GetStream(); $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n"); $st.Write($b, 0, $b.Length)
    $c.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
    $t = [IO.StreamReader]::new($st).ReadToEnd(); $c.Close(); $t
}
# Send the client back to A when a check moved it.
function BackToA { if ((Att A) -ne '1') { P switch-client -t A | Out-Null; [void](WaitFor { (Att A) -eq '1' }) } }

Write-Host "`n=== shared routing between sessions (#798 step 3 item 1) ===" -ForegroundColor Cyan
$proc = $null
try {
    P new-session -d -s A -x 120 -y 30 | Out-Null
    P new-session -d -s B -x 120 -y 30 | Out-Null
    P new-session -d -s D -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1500
    $pa = (P display -p -t A '#{pid}') -join ''; $pb = (P display -p -t B '#{pid}') -join ''
    Check ($pa -and $pa -eq $pb) "A and B share one server" "A=$pa B=$pb"

    # 1. hooks naming a sibling
    P set-hook -t A after-split-window 'new-window -d -t B: -n HOOKED' | Out-Null
    P split-window -d -t A | Out-Null
    $ping = PingMs
    Check (WaitFor { (Wn B) -match 'HOOKED' }) "a hook's new-window -t B: creates the window in B" "B=$(Wn B)"
    Check ((Wn A) -notmatch 'HOOKED') "and not in A" "A=$(Wn A)"
    Check ($ping -lt 1000) "the loop answered right after the hook ($ping ms)" "$ping ms"
    P set-hook -t A after-rename-window 'rename-window -t B:0 HOOKREN' | Out-Null
    P rename-window -t A:0 renamedA | Out-Null
    Check (WaitFor { (Wn B) -match 'HOOKREN' }) "a hook's rename-window -t B:0 renames B's window" "B=$(Wn B)"
    Check ((Wn A) -eq 'renamedA') "and leaves A's" "A=$(Wn A)"

    # 2. one shot lines naming a sibling, sent to A's listener
    $r = Raw A 'rename-window -t B:0 RAWNAMED'
    Check ($r -notmatch 'ERROR') "rename-window -t B:0 on A's listener is not refused" ($r -replace "`n", ' / ')
    Check (WaitFor { (Wn B) -match 'RAWNAMED' }) "and renames B's window" "B=$(Wn B)"
    $r = Raw A 'display-message -p -t B: "#{session_name}"'
    Check ($r -match '(?m)^B\s*$') "display-message -p -t B: on A's listener expands in B" ($r -replace "`n", ' / ')
    $r = Raw A 'kill-session -t D'
    Check (WaitFor { -not (Has D) }) "kill-session -t D on A's listener ends D" ($r -replace "`n", ' / ')
    Check (Has A) "and A is alive" ""
    # a one shot copy-mode (the router refused it, the dispatcher had its arms)
    P copy-mode -t "=B:" | Out-Null
    $mode = (P display -p -t B '#{pane_in_mode}') -join ''
    Check ($mode -eq '1') "a one shot copy-mode -t =B: enters copy mode" "pane_in_mode=$mode"
    P send-keys -t B -X cancel | Out-Null
    $r = Raw A 'new-window -d -t C9: -n NOWHERE'
    Check ($r -match "can't find session") "a target naming no session is still tmux's error" ($r -replace "`n", ' / ')

    # 3. switch-client to a window or pane of a sibling
    P new-window -d -t B: -n SW | Out-Null
    P split-window -d -t B:SW | Out-Null
    $proc = Start-Process -FilePath $Exe -ArgumentList "-L", $ns, "attach", "-t", "A" -PassThru
    Check (WaitFor { (Att A) -eq '1' } 8000) "a client is attached to A" "A=$(Att A)"
    Start-Sleep -Milliseconds 800

    $o = (P switch-client -t B:SW.1) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -eq 0) "switch-client -t B:SW.1 exits 0" "rc=$rc $o"
    Check (WaitFor { (Att B) -eq '1' -and (Att A) -eq '0' }) "the client moved to B" "A=$(Att A) B=$(Att B)"
    $w = (P display -p -t B '#{window_name}.#{pane_index}') -join ''
    Check ($w -eq 'SW.1') "B's current window and pane are SW.1" $w
    BackToA

    $o = (P switch-client -t B:NOPE) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -ne 0 -and $o -match "can't find window") "switch-client -t B:NOPE fails with can't find window" "rc=$rc $o"
    Check ((Att A) -eq '1') "and the client stays in A" "A=$(Att A)"
    $bwin = (P display -p -t B '#{window_name}') -join ''
    Check ($bwin -eq 'SW') "and B's current window did not change" $bwin

    P select-window -t B:0 | Out-Null
    $paneId = (P display -p -t B:SW.0 '#{pane_id}') -join ''
    P bind -n F7 switch-client -t $paneId | Out-Null
    Start-Sleep -Milliseconds 500
    & $injector $proc.Id "{F7}" | Out-Null
    Check (WaitFor { (Att B) -eq '1' }) "a binding's switch-client -t $paneId (a pane of B) moves the client to B" "A=$(Att A) B=$(Att B)"
    $now = (P display -p -t B '#{pane_id}') -join ''
    Check ($now -eq $paneId) "and makes $paneId B's current pane" $now
    BackToA

    $winId = (P display -p -t B:0 '#{window_id}') -join ''
    $o = (P switch-client -t $winId) -join ' '
    Check (WaitFor { (Att B) -eq '1' }) "switch-client -t $winId (a window of B) moves the client to B" "rc=$LASTEXITCODE $o"
    $now = (P display -p -t B '#{window_id}') -join ''
    Check ($now -eq $winId) "and selects $winId there" $now
    BackToA
    Check ((PingMs) -lt 1000) "the loop is responsive after the switches" ""

    # 4. new-session from a key binding and from the command prompt
    P bind -n F6 new-session -d -s FROMKEY | Out-Null
    Start-Sleep -Milliseconds 500
    & $injector $proc.Id "{F6}" | Out-Null
    Check (WaitFor { Has FROMKEY } 6000) "a binding's new-session -d -s FROMKEY makes the session" ""
    $pk = (P display -p -t FROMKEY '#{pid}') -join ''
    Check ($pk -eq $pa) "in the same shared server" "pid=$pk server=$pa"
    Check ((Att A) -eq '1') "and with -d the client stays in A" "A=$(Att A)"

    & $injector $proc.Id "^b{SLEEP:400}:{SLEEP:300}new-session -d -s FROMPROMPT{ENTER}" | Out-Null
    Check (WaitFor { Has FROMPROMPT } 6000) "the prompt's new-session -d -s FROMPROMPT makes the session" ""

    & $injector $proc.Id "^b{SLEEP:400}:{SLEEP:300}new-session -s FROMPROMPT2{ENTER}" | Out-Null
    Check (WaitFor { Has FROMPROMPT2 } 6000) "the prompt's new-session -s FROMPROMPT2 makes the session" ""
    Check (WaitFor { (Att FROMPROMPT2) -eq '1' } 6000) "and without -d the client moves to it" "A=$(Att A) FROMPROMPT2=$(Att FROMPROMPT2)"
    BackToA

    # 5. the session chooser still switches
    & $injector $proc.Id "^b{SLEEP:400}s{SLEEP:900}" | Out-Null
    & $injector $proc.Id "j{SLEEP:300}{ENTER}" | Out-Null
    Check (WaitFor { (Att A) -eq '0' } 6000) "choose-tree -s moves the client to the session chosen" "A=$(Att A) B=$(Att B)"
    BackToA

    # 6. still refused, with the new wording: new-session from a hook
    P set -g display-time 8000 | Out-Null
    P set-hook -t A after-select-window 'new-session -d -s FROMHOOK' | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    P new-window -t A | Out-Null
    P select-window -t A:0 | Out-Null
    $ms = $sw.ElapsedMilliseconds
    Start-Sleep -Milliseconds 300
    $dump = (P dump-state -t A) -join ''
    $sm = if ($dump -match '"status_message":"([^"]*)"') { $Matches[1] } else { '' }
    Check (-not (Has FROMHOOK)) "a hook's new-session is still not run" ""
    Check ($sm -match 'new-session from a hook') "and says so on A's status line" "'$sm'"
    Check ($ms -lt 2000) "without stalling the loop ($ms ms)" "$ms ms"
} finally {
    P kill-server | Out-Null
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    foreach ($k in $saved.Keys) {
        if ($null -eq $saved[$k]) { Remove-Item "Env:\$k" -EA SilentlyContinue } else { Set-Item "Env:\$k" $saved[$k] }
    }
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
}

$keepData = $env:PSMUX_DATA_DIR
Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
$defaultAfter = (& $Exe ls 2>&1 | Out-String).Trim()
if ($keepData) { $env:PSMUX_DATA_DIR = $keepData }
if ($defaultAfter -eq $defaultBefore) { Write-Pass "the default namespace is unchanged" } else { Write-Fail "default namespace changed: '$defaultBefore' -> '$defaultAfter'" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Passed)"
Write-Host "  Failed: $($script:Failed)"
exit $script:Failed
