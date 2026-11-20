# #798: several shared servers side by side, one per -L namespace, each holding
# several sessions, next to a namespace in the default (per session) mode.
#
# The owner wants both shapes at once: one server holding many sessions, and
# many servers (namespaces) each holding many sessions. This suite measures that
# end to end on one data directory, the way a real user shares ~/.psmux:
#   * NS1 and NS2 in shared mode, three sessions each, with the SAME names
#     (a, b, c) plus one name unique to each namespace
#   * NS3 in the default mode with two sessions
#   * create, list-sessions, has-session, send-keys, capture-pane, kill-session,
#     attach, switch-client, new-window, rename-window and kill-server in each,
#     checking after every step that the other namespaces saw nothing
#   * registry isolation: one marker per shared namespace, distinct ports and
#     keys, a session's files carry its own namespace's prefix, and killing one
#     namespace's server removes only that namespace's files
#
# Isolation: unique -L names, a private PSMUX_DATA_DIR, an empty config;
# cleanup only with `-L <ns> kill-server` and the client windows by PID.
# Pass -Exe or set PSMUX_TEST_EXE (PSMUX_TEST_BIN is honoured too).
param([string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }))

$ErrorActionPreference = "Continue"
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $m, $detail) { if ($ok) { Write-Pass $m } else { Write-Fail "$m ($detail)" } }

$saved = @{ PSMUX_DATA_DIR = $env:PSMUX_DATA_DIR; PSMUX_SERVER_MODE = $env:PSMUX_SERVER_MODE; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE; PSMUX_RUN_NOPAUSE = $env:PSMUX_RUN_NOPAUSE }
foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME') { Remove-Item "Env:\$v" -EA SilentlyContinue }
Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
Remove-Item Env:\PSMUX_SERVER_MODE -EA SilentlyContinue
$defaultBefore = (& $Exe ls 2>&1 | Out-String).Trim()

$tag = Get-Random
$NS1 = "mnsA$tag"; $NS2 = "mnsB$tag"; $NS3 = "mnsD$tag"
$data = Join-Path $env:TEMP "psmux_mns$tag"
New-Item -ItemType Directory $data -Force | Out-Null
$env:PSMUX_DATA_DIR = $data
$env:PSMUX_RUN_NOPAUSE = "1"
$empty = Join-Path $data "empty.conf"
"" | Set-Content $empty
$env:PSMUX_CONFIG_FILE = $empty

# Run a command in a namespace, in shared mode or with the switch unset.
function PS1 { $env:PSMUX_SERVER_MODE = "shared"; & $Exe -L $NS1 @args 2>&1 }
function PS2 { $env:PSMUX_SERVER_MODE = "shared"; & $Exe -L $NS2 @args 2>&1 }
function PD3 { Remove-Item Env:\PSMUX_SERVER_MODE -EA SilentlyContinue; & $Exe -L $NS3 @args 2>&1 }
function InNs($ns) { switch ($ns) { $NS1 { 'PS1' } $NS2 { 'PS2' } $NS3 { 'PD3' } } }
function Q($ns) { $f = InNs $ns; & $f @args }
function Pid($ns, $s) { ((Q $ns display -p -t $s '#{pid}') -join '').Trim() }
function Sessions($ns) { ((Q $ns list-sessions -F '#{session_name}') | Sort-Object) -join ',' }
function Has($ns, $s) { Q $ns has-session -t "=$s" | Out-Null; $LASTEXITCODE -eq 0 }
function Att($ns, $s) { ((Q $ns display -p -t $s '#{session_attached}') -join '').Trim() }
function Wn($ns, $s) { ((Q $ns list-windows -t $s -F '#{window_name}') -join ',') }
function Cap($ns, $s) { (Q $ns capture-pane -p -t $s) -join "`n" }
function WaitFor([scriptblock]$cond, [int]$ms = 6000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
function RegFile($ns, $s, $ext) { Join-Path $data "${ns}__$s.$ext" }
function RegVal($ns, $s, $ext) { $p = RegFile $ns $s $ext; if (Test-Path $p) { (Get-Content $p -Raw).Trim() } else { '' } }
function Marker($ns) { Join-Path $data "shared\L-$ns.server" }
# Every registry file name of a namespace (the base and its extension).
function NsFiles($ns) { @(Get-ChildItem $data -File -EA SilentlyContinue | Where-Object { $_.Name -like "${ns}__*" } | ForEach-Object Name | Sort-Object) }

$clients = @()
Write-Host "`n=== #798: shared servers in several namespaces, next to a default one ===" -ForegroundColor Cyan
try {
    # ---- create ----
    foreach ($s in 'a', 'b', 'c', 'only1') { PS1 new-session -d -s $s -x 120 -y 30 | Out-Null }
    foreach ($s in 'a', 'b', 'c', 'only2') { PS2 new-session -d -s $s -x 120 -y 30 | Out-Null }
    foreach ($s in 'x', 'y') { PD3 new-session -d -s $s -x 120 -y 30 | Out-Null }
    Check (WaitFor { (Sessions $NS1) -eq 'a,b,c,only1' }) "NS1 lists exactly its four sessions" (Sessions $NS1)
    Check (WaitFor { (Sessions $NS2) -eq 'a,b,c,only2' }) "NS2 lists exactly its four sessions" (Sessions $NS2)
    Check (WaitFor { (Sessions $NS3) -eq 'x,y' }) "NS3 (default mode) lists exactly its two sessions" (Sessions $NS3)

    # ---- one process per shared namespace, one per session in the default one ----
    $p1 = @('a', 'b', 'c', 'only1' | ForEach-Object { Pid $NS1 $_ })
    $p2 = @('a', 'b', 'c', 'only2' | ForEach-Object { Pid $NS2 $_ })
    $p3 = @('x', 'y' | ForEach-Object { Pid $NS3 $_ })
    Check ($p1[0] -and @($p1 | Select-Object -Unique).Count -eq 1) "all NS1 sessions share one server process" ($p1 -join ',')
    Check ($p2[0] -and @($p2 | Select-Object -Unique).Count -eq 1) "all NS2 sessions share one server process" ($p2 -join ',')
    Check ($p1[0] -ne $p2[0]) "NS1 and NS2 are different processes" "$($p1[0]) vs $($p2[0])"
    Check ($p3[0] -and $p3[1] -and $p3[0] -ne $p3[1]) "NS3 runs one server per session (default mode untouched)" ($p3 -join ',')
    Check (($p3 -notcontains $p1[0]) -and ($p3 -notcontains $p2[0])) "and none of them is a shared server" ($p3 -join ',')
    $procs = @($p1[0], $p2[0]) + $p3 | ForEach-Object { Get-Process -Id $_ -EA SilentlyContinue }
    Check ($procs.Count -eq 4) "four server processes in all (2 shared + 2 default)" "$($procs.Count)"

    # ---- registry isolation ----
    Check ((Test-Path (Marker $NS1)) -and (Test-Path (Marker $NS2))) "each shared namespace has its own marker" "NS1=$(Test-Path (Marker $NS1)) NS2=$(Test-Path (Marker $NS2))"
    Check (-not (Test-Path (Marker $NS3))) "the default namespace has no shared marker" ""
    $m1 = Get-Content (Marker $NS1) -Raw; $m2 = Get-Content (Marker $NS2) -Raw
    Check ($m1 -match "pid=$($p1[0])" -and $m2 -match "pid=$($p2[0])") "each marker names its own server" ""
    $ports = @(); $keys = @()
    foreach ($pair in @(@($NS1, 'a'), @($NS1, 'b'), @($NS1, 'c'), @($NS2, 'a'), @($NS2, 'b'), @($NS2, 'c'), @($NS3, 'x'), @($NS3, 'y'))) {
        $ports += RegVal $pair[0] $pair[1] 'port'; $keys += RegVal $pair[0] $pair[1] 'key'
    }
    Check (@($ports | Where-Object { $_ }).Count -eq 8 -and @($ports | Select-Object -Unique).Count -eq 8) "every session in every namespace has its own port" ($ports -join ',')
    Check (@($keys | Where-Object { $_ }).Count -eq 8 -and @($keys | Select-Object -Unique).Count -eq 8) "and its own key" "$(@($keys | Select-Object -Unique).Count) unique"
    # A .pid file holds `<pid>:<creation time>`.
    $pf1 = (RegVal $NS1 'a' 'pid') -replace ':.*$', ''; $pf2 = (RegVal $NS2 'a' 'pid') -replace ':.*$', ''
    Check ($pf1 -eq $p1[0] -and $pf2 -eq $p2[0]) "the same session name a has a separate pid file per namespace, naming its own server" "NS1 a=$pf1 NS2 a=$pf2"

    # ---- name resolution never crosses ----
    Check (-not (Has $NS2 'only1')) "NS2 cannot find NS1's only1" ""
    Check (-not (Has $NS1 'only2')) "NS1 cannot find NS2's only2" ""
    Check (-not (Has $NS3 'a')) "the default namespace cannot find a shared session" ""
    Check (-not (Has $NS1 'x')) "a shared namespace cannot find a default session" ""
    # The CLI answers a session it cannot find the same way in both modes
    # ("no server running on session ..."), so only the outcome is checked.
    $o = (PS2 new-window -d -t only1: -n STRAY) -join ' '; $rc = $LASTEXITCODE
    Check ($rc -ne 0) "new-window -t only1: in NS2 fails" "rc=$rc $o"
    Check (((Wn $NS1 'only1') -notmatch 'STRAY') -and ((Wn $NS2 'a') -notmatch 'STRAY')) "and creates no window in either namespace" ""

    # ---- send-keys / capture with identical names ----
    PS1 send-keys -t a 'echo NS1MARK' Enter | Out-Null
    PS2 send-keys -t a 'echo NS2MARK' Enter | Out-Null
    PD3 send-keys -t x 'echo NS3MARK' Enter | Out-Null
    Check (WaitFor { (Cap $NS1 'a') -match 'NS1MARK' }) "send-keys to NS1 a shows in NS1 a" ""
    Check (WaitFor { (Cap $NS2 'a') -match 'NS2MARK' }) "send-keys to NS2 a shows in NS2 a" ""
    Check (WaitFor { (Cap $NS3 'x') -match 'NS3MARK' }) "send-keys to NS3 x shows in NS3 x" ""
    $c1 = Cap $NS1 'a'; $c2 = Cap $NS2 'a'
    Check ($c1 -notmatch 'NS2MARK' -and $c2 -notmatch 'NS1MARK') "neither a sees the other's keys" ""
    Check (((Cap $NS1 'b') -notmatch 'NS1MARK')) "NS1 b did not get a's keys" ""

    # ---- window ops in one namespace leave the twin alone ----
    PS1 new-window -d -t b: -n W1 | Out-Null
    PS1 rename-window -t b:0 R1 | Out-Null
    Check (WaitFor { (Wn $NS1 'b') -eq 'R1,W1' }) "NS1 b got its window and rename" (Wn $NS1 'b')
    $w2 = Wn $NS2 'b'
    Check ($w2 -notmatch 'W1|R1') "NS2 b is unchanged" $w2

    # ---- attach and switch-client per namespace ----
    $env:PSMUX_SERVER_MODE = "shared"
    $cl1 = Start-Process -FilePath $Exe -ArgumentList "-L", $NS1, "attach", "-t", "a" -PassThru
    $cl2 = Start-Process -FilePath $Exe -ArgumentList "-L", $NS2, "attach", "-t", "a" -PassThru
    $clients += $cl1, $cl2
    Check (WaitFor { (Att $NS1 'a') -eq '1' } 8000) "a client attaches to NS1 a" "NS1 a=$(Att $NS1 'a')"
    Check (WaitFor { (Att $NS2 'a') -eq '1' } 8000) "a client attaches to NS2 a" "NS2 a=$(Att $NS2 'a')"
    Start-Sleep -Milliseconds 600
    $o = (PS1 switch-client -t c) -join ' '
    Check (WaitFor { (Att $NS1 'c') -eq '1' -and (Att $NS1 'a') -eq '0' }) "switch-client -t c moves NS1's client to c" "rc=$LASTEXITCODE $o a=$(Att $NS1 'a') c=$(Att $NS1 'c')"
    Check ((Att $NS2 'a') -eq '1' -and (Att $NS2 'c') -eq '0') "NS2's client stays on a" "a=$(Att $NS2 'a') c=$(Att $NS2 'c')"
    $o = (PD3 switch-client -t y) -join ' '; $rc = $LASTEXITCODE
    Check ((Att $NS1 'c') -eq '1' -and (Att $NS2 'a') -eq '1') "switch-client in the default namespace moves no shared client" "rc=$rc $o"
    $o = (PS2 switch-client -t b) -join ' '
    Check (WaitFor { (Att $NS2 'b') -eq '1' }) "switch-client -t b in NS2 moves NS2's client" "$o"
    Check ((Att $NS1 'c') -eq '1' -and (Att $NS1 'b') -eq '0') "NS1's client is still on c" "c=$(Att $NS1 'c') b=$(Att $NS1 'b')"
    $lc1 = ((PS1 list-sessions -F '#{session_name}:#{session_attached}') | Sort-Object) -join ','
    $lc2 = ((PS2 list-sessions -F '#{session_name}:#{session_attached}') | Sort-Object) -join ','
    Check ($lc1 -eq 'a:0,b:0,c:1,only1:0') "NS1 list-sessions shows its one client on c" $lc1
    Check ($lc2 -eq 'a:0,b:1,c:0,only2:0') "NS2 list-sessions shows its one client on b" $lc2

    # ---- kill-session in one namespace ----
    $b1Files = @(NsFiles $NS1 | Where-Object { $_ -like "*__b.*" })
    PS1 kill-session -t b | Out-Null
    Check (WaitFor { -not (Has $NS1 'b') }) "kill-session -t b ends NS1 b" ""
    Check ((Has $NS2 'b') -and (Att $NS2 'b') -eq '1') "NS2 b is alive with its client" ""
    Check (WaitFor { @(NsFiles $NS1 | Where-Object { $_ -like "*__b.*" }).Count -eq 0 }) "NS1 b's registry files are gone" ((NsFiles $NS1) -join ',')
    Check (@(NsFiles $NS2 | Where-Object { $_ -like "*__b.*" }).Count -ge 3) "NS2 b's registry files remain" ((NsFiles $NS2) -join ',')
    Check ((Pid $NS1 'a') -eq $p1[0]) "the NS1 server is the same process after the kill" ""
    Check ($b1Files.Count -ge 3) "(NS1 b had its files before: $($b1Files -join ','))" ""

    # ---- new session in a shared namespace joins its own server ----
    PS2 new-session -d -s late -x 100 -y 25 | Out-Null
    Check ((Pid $NS2 'late') -eq $p2[0]) "a new NS2 session joins the NS2 server" "late=$(Pid $NS2 'late') server=$($p2[0])"
    Check (-not (Has $NS1 'late')) "and is not visible in NS1" ""

    # ---- kill-server of one shared namespace ----
    PS1 kill-server | Out-Null
    Check (WaitFor { -not (Get-Process -Id $p1[0] -EA SilentlyContinue) }) "NS1 kill-server ends the NS1 process" ""
    Check (WaitFor { (NsFiles $NS1).Count -eq 0 -and -not (Test-Path (Marker $NS1)) }) "and removes every NS1 file and its marker" ((NsFiles $NS1) -join ',')
    Check ((Sessions $NS2) -eq 'a,b,c,late,only2') "NS2 keeps all its sessions" (Sessions $NS2)
    Check ((Test-Path (Marker $NS2)) -and (Get-Process -Id $p2[0] -EA SilentlyContinue)) "NS2's server and marker are intact" ""
    Check ((Sessions $NS3) -eq 'x,y' -and (Pid $NS3 'x') -eq $p3[0]) "NS3 (default) is untouched" (Sessions $NS3)
    PS2 send-keys -t c 'echo STILLHERE' Enter | Out-Null
    Check (WaitFor { (Cap $NS2 'c') -match 'STILLHERE' }) "NS2 still serves send-keys and capture-pane" ""

    # ---- a namespace can be reused in shared mode after its server ended ----
    PS1 new-session -d -s again -x 100 -y 25 | Out-Null
    $pa = Pid $NS1 'again'
    Check ($pa -and $pa -ne $p1[0] -and $pa -ne $p2[0]) "NS1 starts a fresh shared server" "pid=$pa"
    Check ((Test-Path (Marker $NS1)) -and ((Get-Content (Marker $NS1) -Raw) -match "pid=$pa")) "with a fresh marker" ""
} finally {
    foreach ($c in $clients) { if ($c -and -not $c.HasExited) { Stop-Process -Id $c.Id -Force -EA SilentlyContinue } }
    PS1 kill-server | Out-Null
    PS2 kill-server | Out-Null
    PD3 kill-server | Out-Null
    foreach ($k in $saved.Keys) {
        if ($null -eq $saved[$k]) { Remove-Item "Env:\$k" -EA SilentlyContinue } else { Set-Item "Env:\$k" $saved[$k] }
    }
    Start-Sleep -Milliseconds 500
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
