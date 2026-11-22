# Issue #761: the server exited with the attached client after answering a
# pane query.
#
# Every console injection (a query reply, a mouse record, a console mode
# probe) does FreeConsole, AttachConsole(pane child), the write, FreeConsole,
# and then AttachConsole(ATTACH_PARENT_PROCESS) when the process had a console
# before.  spawn_server_hidden starts the server with CREATE_NEW_CONSOLE, so
# the server always "had" one, and the first injection moved it into the
# console of its PARENT, which for a cold start is the client that spawned it.
# Over ssh that console is the pseudoconsole sshd made for the channel, so
# when the connection closed the server got CTRL_CLOSE_EVENT with the client
# and took every session with it.  Returning TRUE from a handler does not save
# a process from CTRL_CLOSE_EVENT.
#
# tmux never shares a terminal with the client it was started from: the server
# daemonizes (proc_fork_and_daemon, daemon(3)) and detaches from the
# controlling terminal before it runs a single command, so a client hanging up
# can only ever take the client down.
#
# The rig emulates ssh without ssh: tests\conpty_ctrlc_host.cs hosts the client
# under a real pseudoconsole and QUIT calls ClosePseudoConsole, which is what
# sshd does when the channel goes away.  tests\console_members.cs lists who is
# attached to the client's console, so the defect is seen directly and not
# only through its consequence.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE', 'PSMUX_CONFIG_FILE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i761_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
# Our own data dir: port files, the registry and logs never touch ~/.psmux,
# and a server from this suite cannot be confused with one of the user's.
$env:PSMUX_DATA_DIR = Join-Path $TMP "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$HOSTEXE = Join-Path $TMP "conpty_host.exe"
$MEMBERS = Join-Path $TMP "console_members.exe"
$PROBE   = Join-Path $TMP "query_probe.exe"
foreach ($pair in @(@($HOSTEXE, "conpty_ctrlc_host.cs"), @($MEMBERS, "console_members.cs"), @($PROBE, "query_probe_child.cs"))) {
    if ($csc -and (Test-Path $csc)) {
        & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $PSScriptRoot $pair[1]) 2>&1 | Out-Null
    }
    if (-not (Test-Path $pair[0])) {
        Write-Fail "could not build tests\$($pair[1]) (csc.exe unavailable)"
        Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
        exit 1
    }
}

Add-Type -Namespace I761 -Name Win -MemberDefinition @'
public delegate bool EnumProc(System.IntPtr h, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
'@ -EA SilentlyContinue

# ---------------------------------------------------------------- helpers

# A hosted client: the client runs under its own pseudoconsole, like sshd runs
# the remote command of `ssh -t`.
function Start-HostedClient([string]$ns, [string]$cliArgs) {
    $dir = Join-Path $TMP ("host_" + [guid]::NewGuid().ToString('N').Substring(0, 6))
    New-Item -ItemType Directory -Force $dir | Out-Null
    $old = $env:TEMP; $env:TEMP = $dir
    try {
        $h = Start-Process $HOSTEXE -ArgumentList "`"$PSMUX`" -L $ns $cliArgs" -PassThru -WindowStyle Hidden
    } finally { $env:TEMP = $old }
    $script:Opened += $h.Id
    $client = $null
    for ($i = 0; $i -lt 100 -and -not $client; $i++) {
        Start-Sleep -Milliseconds 100
        $f = Join-Path $dir "conpty_childpid.txt"
        if (Test-Path $f) { $t = (Get-Content $f -Raw -EA SilentlyContinue); if ($t) { $client = [int]$t.Trim() } }
    }
    if ($client) { $script:Opened += $client }
    [pscustomobject]@{ Host = $h; Client = $client; Dir = $dir }
}

# ClosePseudoConsole: the channel going away.
function Close-HostedClient($hc) {
    Add-Content -Path (Join-Path $hc.Dir "conpty_ctrl.txt") -Value "QUIT"
    for ($i = 0; $i -lt 50; $i++) {
        if (-not (Get-Process -Id $hc.Host.Id -EA SilentlyContinue)) { break }
        Start-Sleep -Milliseconds 100
    }
    for ($i = 0; $i -lt 50; $i++) {
        if (-not (Get-Process -Id $hc.Client -EA SilentlyContinue)) { break }
        Start-Sleep -Milliseconds 100
    }
}

function Get-ConsoleMembers([int]$attachedPid) {
    $out = Join-Path $TMP ("members_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".txt")
    Start-Process $MEMBERS -ArgumentList $attachedPid, "`"$out`"" -WindowStyle Hidden -Wait
    if (Test-Path $out) { $r = (Get-Content $out -Raw).Trim(); Remove-Item $out -EA SilentlyContinue; return $r }
    return "NOFILE"
}
function Test-Member([string]$members, [int]$p) { return (" $members " -match " $p ") }

function Wait-Prompt([string]$ns, [string]$target) {
    for ($i = 0; $i -lt 150; $i++) {
        $c = (& $PSMUX -L $ns capture-pane -p -t $target 2>$null) -join "`n"
        if ($c -match 'PS [A-Z]:\\') { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
function Wait-Capture([string]$ns, [string]$target, [string]$pattern, [int]$ms = 8000) {
    $deadline = [DateTime]::Now.AddMilliseconds($ms)
    while ([DateTime]::Now -lt $deadline) {
        $c = (& $PSMUX -L $ns capture-pane -p -J -t $target 2>$null) -join "`n"
        if ($c -match $pattern) { return $c }
        Start-Sleep -Milliseconds 150
    }
    return $null
}

function Get-ServerPid([string]$ns, [string]$target) {
    for ($i = 0; $i -lt 150; $i++) {
        $o = & $PSMUX -L $ns display-message -p -t $target '#{pid}' 2>$null
        if ($LASTEXITCODE -eq 0 -and "$o".Trim() -match '^\d+$') { return [int]"$o".Trim() }
        Start-Sleep -Milliseconds 100
    }
    return $null
}

function Get-Descendants([int]$root) {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
    $set = @{ $root = $true }; $grew = $true
    while ($grew) {
        $grew = $false
        foreach ($p in $all) { if ($set.ContainsKey([int]$p.ParentProcessId) -and -not $set.ContainsKey([int]$p.ProcessId)) { $set[[int]$p.ProcessId] = $true; $grew = $true } }
    }
    $set.Remove($root); return @($set.Keys)
}
function Get-VisibleWindowPids {
    $script:vis = @()
    [I761.Win]::EnumWindows({ param($h, $l) $p = 0; [void][I761.Win]::GetWindowThreadProcessId($h, [ref]$p); if ([I761.Win]::IsWindowVisible($h)) { $script:vis += [int]$p }; $true }, [IntPtr]::Zero) | Out-Null
    return $script:vis
}

# The query exactly as the reporter typed it.
$QUERY = "[Console]::Write([char]27+'[>q')"

# Proof that a reply really is injected: tests\query_probe_child.cs puts the
# console in raw VT input mode, asks XTVERSION and logs the bytes that came
# back.  The pwsh one liner above cannot read its own reply because the
# console is in cooked mode.  The probe runs as the command of a window of its
# own: it leaves the console in raw mode, so a shell it ran under would stop
# taking typed commands.  The Windows 10 inbox conhost (build 19045) hands
# psmux a query only when it next draws something, so there the probe writes a
# letter after each query (see query_probe_child.cs).
$PAINT = if ([Environment]::OSVersion.Version.Build -lt 22000) { " paint" } else { "" }
function Invoke-ProbeIn([string]$ns, [string]$session) {
    $log = Join-Path $TMP ("probe_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".txt")
    # $TMP has no spaces (a guid under the temp dir), so no quoting, the way
    # tests\test_issue597_xtversion_reply.ps1 starts the same probe.
    & $PSMUX -L $ns new-window -d -t $session "$PROBE $log 900$PAINT" 2>&1 | Out-Null
    $deadline = [DateTime]::Now.AddSeconds(30)
    while ([DateTime]::Now -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
    if (-not (Test-Path $log)) { return "" }
    foreach ($line in (Get-Content $log)) { if ($line.StartsWith("XTVERSION ")) { return $line } }
    return ""
}
function Test-XtReply([string]$line) { return ($line -match 'got=(\d+) bytes' -and [int]$Matches[1] -gt 0 -and $line -match 'tmux') }
function Cleanup-Ns([string]$ns) { & $PSMUX -L $ns kill-server 2>&1 | Out-Null }

# ---------------------------------------------------------------- tests

$defaultBefore = (& $PSMUX ls 2>&1) -join "`n"
$allNs = @()

Write-Head "Test 1: a cold server is not in the client's console after it answers XTVERSION"
$NS = "a761s" + [guid]::NewGuid().ToString('N').Substring(0, 8); $allNs += $NS
& $PSMUX -L $NS ls 2>&1 | Out-Null
Check ($LASTEXITCODE -ne 0) "namespace $NS is cold" "namespace $NS already had a server"
$hc = Start-HostedClient $NS "new-session -A -s t1"
$SRV = Get-ServerPid $NS "t1"
Check ($hc.Client -and $SRV) "client $($hc.Client) and server $SRV are up" "client or server did not start (client=$($hc.Client) server=$SRV)"
$srvParent = (Get-CimInstance Win32_Process -Filter "ProcessId=$SRV" -EA SilentlyContinue).ParentProcessId
Check ($srvParent -eq $hc.Client) "server was cold spawned by this client (parent $srvParent)" "server parent is $srvParent, not the hosted client $($hc.Client)"
[void](Wait-Prompt $NS "t1")
$m0 = Get-ConsoleMembers $hc.Client
Check (-not (Test-Member $m0 $SRV)) "before the query the server is not in the client console [$m0]" "server already in the client console before any injection [$m0]"
& $PSMUX -L $NS send-keys -t t1 $QUERY Enter
Start-Sleep -Milliseconds 1500
$m1a = Get-ConsoleMembers $hc.Client
Check (-not (Test-Member $m1a $SRV)) "after the reporter's query the server is not in the client console [$m1a]" "server $SRV joined the client console after the reporter's query [$m1a] (#761)"
$xt = Invoke-ProbeIn $NS "t1"
Check (Test-XtReply $xt) "the pane received the XTVERSION reply through console injection: $xt" "no XTVERSION reply reached the pane: [$xt]"
$m1 = Get-ConsoleMembers $hc.Client
Check (-not (Test-Member $m1 $SRV)) "after the injection the server is still not in the client console [$m1]" "server $SRV joined the client console after the injection [$m1] (#761)"

Write-Head "Test 2: closing the client's pseudoconsole leaves the server running"
Close-HostedClient $hc
Start-Sleep -Milliseconds 1500
Check (-not (Get-Process -Id $hc.Client -EA SilentlyContinue)) "the client exited with its console" "client $($hc.Client) still running after its console closed"
Check ([bool](Get-Process -Id $SRV -EA SilentlyContinue)) "server $SRV survived the client disconnect" "server $SRV died with the client (#761)"
$ls = (& $PSMUX -L $NS ls 2>&1) -join ' '
Check ($ls -match '^t1:') "ls still lists the session: $ls" "session gone: $ls"

Write-Head "Test 3: a server without a console still does its work"
$visBefore = Get-VisibleWindowPids
$w = ((& $PSMUX -L $NS new-window -t t1 -P -F '#{pane_id}' 2>&1) -join '').Trim()
$sp = ((& $PSMUX -L $NS split-window -t $w -P -F '#{pane_id}' 2>&1) -join '').Trim()
$panes = @(& $PSMUX -L $NS list-panes -t $sp -F '#{pane_id}' 2>&1 | Where-Object { "$_" -match '^%\d+$' }).Count
Check ($panes -eq 2 -and $w -match '^%\d+$' -and $sp -match '^%\d+$') "new-window ($w) and split-window ($sp) after the injection: the new window has 2 panes" "expected 2 panes in the new window, got $panes (new [$w] split [$sp])"
[void](Wait-Prompt $NS $sp)
& $PSMUX -L $NS send-keys -t $sp "echo ALIVE$('761')" Enter
Check (Wait-Capture $NS $sp 'ALIVE761') "a new pane runs commands" "new pane did not echo"
$xt2 = Invoke-ProbeIn $NS "t1"
Check (Test-XtReply $xt2) "an injection still works from the console-less server: $xt2" "second XTVERSION reply missing: [$xt2]"
& $PSMUX -L $NS send-keys -t $sp "clear" Enter
Start-Sleep -Milliseconds 800
& $PSMUX -L $NS send-keys -t $sp "ping -t 127.0.0.1" Enter
$pinging = Wait-Capture $NS $sp 'Reply from 127\.0\.0\.1' 8000
Start-Sleep -Milliseconds 1000
& $PSMUX -L $NS send-keys -t $sp C-c
$c = Wait-Capture $NS $sp 'Control-C|Ping statistics' 8000
Check ($pinging -and $c) "send-keys C-c interrupts ping in a pane" "ping was not interrupted by C-c (pinging=$([bool]$pinging))"
$rs = (& $PSMUX -L $NS run-shell -t t1 "cmd /c echo RS761" 2>&1) -join ' '
Check ($rs -match 'RS761') "run-shell output comes back: $rs" "run-shell output missing: $rs"
$fmt = (& $PSMUX -L $NS display-message -p -t t1 '#(cmd /c echo FMT761)' 2>&1) -join ' '
for ($i = 0; $i -lt 10 -and $fmt -notmatch 'FMT761'; $i++) { Start-Sleep -Milliseconds 300; $fmt = (& $PSMUX -L $NS display-message -p -t t1 '#(cmd /c echo FMT761)' 2>&1) -join ' ' }
Check ($fmt -match 'FMT761') "#() format job output comes back" "#() output missing: $fmt"
Start-Sleep -Milliseconds 500
$desc = Get-Descendants $SRV
$visAfter = Get-VisibleWindowPids
$newVis = @($visAfter | Where-Object { $visBefore -notcontains $_ -and $desc -contains $_ })
Check ($newVis.Count -eq 0) "no visible console window appeared for any server child" "visible windows owned by server children: $($newVis -join ',')"

Write-Head "Test 4: a second client attaches, queries, disconnects, and the server stays"
$hc2 = Start-HostedClient $NS "attach -t t1"
Start-Sleep -Milliseconds 1500
& $PSMUX -L $NS send-keys -t t1 $QUERY Enter
Start-Sleep -Milliseconds 1500
$m2 = Get-ConsoleMembers $hc2.Client
Check (-not (Test-Member $m2 $SRV)) "server is not in the second client's console [$m2]" "server joined the second client's console [$m2]"
Close-HostedClient $hc2
Start-Sleep -Milliseconds 1500
Check ([bool](Get-Process -Id $SRV -EA SilentlyContinue)) "server survived the second disconnect" "server died with the second client"

Write-Head "Test 5: kill-server still tears the detached server down cleanly"
$desc = Get-Descendants $SRV
Cleanup-Ns $NS
for ($i = 0; $i -lt 50 -and (Get-Process -Id $SRV -EA SilentlyContinue); $i++) { Start-Sleep -Milliseconds 100 }
Check (-not (Get-Process -Id $SRV -EA SilentlyContinue)) "server exited on kill-server" "server $SRV still running after kill-server"
Start-Sleep -Milliseconds 1500
$left = @($desc | Where-Object { Get-Process -Id $_ -EA SilentlyContinue })
Check ($left.Count -eq 0) "no pane process outlived the server" "pane processes still alive: $($left -join ',')"
$script:Opened += $left

Write-Head "Test 6: control, no query, the server survives the disconnect"
$NS2 = "a761c" + [guid]::NewGuid().ToString('N').Substring(0, 8); $allNs += $NS2
$hc3 = Start-HostedClient $NS2 "new-session -A -s t1"
$SRV2 = Get-ServerPid $NS2 "t1"
[void](Wait-Prompt $NS2 "t1")
Close-HostedClient $hc3
Start-Sleep -Milliseconds 1500
Check ($SRV2 -and (Get-Process -Id $SRV2 -EA SilentlyContinue)) "control server $SRV2 survived" "control server died without any query"
Cleanup-Ns $NS2

# ---------------------------------------------------------------- cleanup
foreach ($n in $allNs) { Cleanup-Ns $n }
Start-Sleep -Milliseconds 500
foreach ($id in $script:Opened) { if ($id -and (Get-Process -Id $id -EA SilentlyContinue)) { Stop-Process -Id $id -Force -EA SilentlyContinue } }
$leftover = @($script:Opened | Where-Object { $_ -and (Get-Process -Id $_ -EA SilentlyContinue) })
Check ($leftover.Count -eq 0) "every process the suite opened is gone" "still running: $($leftover -join ',')"
$defaultAfter = (& $PSMUX ls 2>&1) -join "`n"
Check ($defaultBefore -eq $defaultAfter) "default namespace untouched" "default namespace changed: before [$defaultBefore] after [$defaultAfter]"
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
