# Issue #1005: an attached -tTARGET (`-t:0`, `-tA:0`, `-tK2`) from a hook, a
# key binding, a mouse binding or the command prompt applied to the ACTIVE
# window or the CURRENT session.
#
# tmux reads `-tVALUE` as `-t VALUE` on every path (arguments.c
# args_parse_flag_argument). The psmux CLI split it before it forwarded
# (main.rs normalize_attached_target_flag), but two server paths did not:
#   - the in server route (hooks, source-file lines, commands.rs
#     execute_unaliased_command): `rename-window -t:0 X` from a hook renamed
#     the active window 1;
#   - the server's command handler (connection.rs), which a client's key
#     binding and command prompt and a raw TCP line reach: `kill-session -tK2`
#     from the prompt killed the client's own session.
#
# GROUND TRUTH: list-windows names and list-sessions after each line, with
# window 1 active and window 0 (or session K2) the target. Every attached form
# is checked next to its spaced form, which already worked.
#
# Layers: hook, source-file, raw TCP to the server handler, CLI, and an
# attached TUI client driven by a bound key and the command prompt
# (WriteConsoleInput through tests\injector.cs).
#
# Isolation: a unique -L namespace and a private PSMUX_DATA_DIR; cleanup is
# `-L <ns> kill-server` and the client window this script opened, by PID.
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') {
    Remove-Item "env:$v" -EA SilentlyContinue
}
$defaultBefore = ((& $PSMUX ls -F '#{session_name}' 2>&1) -join "`n")

$NS  = "t1005_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$TMP = Join-Path ([IO.Path]::GetTempPath()) ("psmux_1005_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$CONF = Join-Path $TMP 'empty.conf'; Set-Content $CONF '' -Encoding ASCII
$env:PSMUX_CONFIG_FILE = $CONF

function P { & $PSMUX -L $NS @args 2>&1 }
function Names($s = 'A') { ((P list-windows -t $s -F '#I=#W') -join ' ') }
function Sessions { (((P ls -F '#S') | Sort-Object) -join ' ') }
function WaitFor($block, $ms = 4000) {
    $end = [DateTime]::Now.AddMilliseconds($ms)
    while ([DateTime]::Now -lt $end) { if (& $block) { return $true }; Start-Sleep -Milliseconds 100 }
    return (& $block)
}

# Session A with windows 0, 1 and 2, window 1 active.
function Reset-A {
    P kill-session -t A | Out-Null
    P new-session -d -s A -x 120 -y 30 | Out-Null
    P new-window -t A: | Out-Null
    P new-window -t A: | Out-Null
    P select-window -t A:1 | Out-Null
    $script:Base = Names
}
# The expected list: window 0 named $name, the others as they were.
function Want0($name) { ($script:Base -replace '^0=\S+', "0=$name") }

# One raw line to the session's command handler, the way an attached client
# sends its binding and prompt commands (no CLI normalisation on the way).
function Send-Tcp($session, $line) {
    $port = (Get-Content (Join-Path $TMP "${NS}__$session.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $TMP "${NS}__$session.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    try {
        $tcp.ReceiveTimeout = 3000
        $st = $tcp.GetStream()
        $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
        $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
        $w.Write("$line`n"); $w.Flush()
        try { $null = $r.ReadToEnd() } catch {}
    } catch {
    } finally { $tcp.Close() }
}

# ======================================================================
Write-Head "Hook (in server route)"
# ======================================================================
foreach ($t in @(':0', 'A:0')) {
    foreach ($form in @("-t $t", "-t$t")) {
        Reset-A
        P set-hook -t A after-new-window "rename-window $form H" | Out-Null
        P new-window -d -t A: | Out-Null
        $want = (Want0 'H') + ' 3=' + ($script:Base -replace '.*\b2=(\S+).*', '$1')
        $ok = WaitFor { (Names) -eq $want }
        P set-hook -u -t A after-new-window | Out-Null
        Check $ok "hook 'rename-window $form H' renamed window 0" "hook 'rename-window $form H' gave '$(Names)', want '$want'"
    }
}

# `-t=A:0` is tmux's `-t =A:0`, an exact match target.
Reset-A
P set-hook -t A after-new-window 'rename-window -t=A:0 E' | Out-Null
P new-window -d -t A: | Out-Null
$ok = WaitFor { (Names) -match '^0=E ' }
P set-hook -u -t A after-new-window | Out-Null
Check $ok "hook 'rename-window -t=A:0 E' renamed window 0" "hook 'rename-window -t=A:0 E' gave '$(Names)'"

# ======================================================================
Write-Head "source-file (in server route)"
# ======================================================================
foreach ($form in @('-t :0', '-t:0', '-tA:0')) {
    Reset-A
    $f = Join-Path $TMP 'src.conf'
    Set-Content $f "rename-window $form S" -Encoding ASCII
    P source-file $f | Out-Null
    $ok = WaitFor { (Names) -eq (Want0 'S') }
    Check $ok "source-file 'rename-window $form S' renamed window 0" "source-file 'rename-window $form S' gave '$(Names)'"
}
# Another arm that reads -t on its own: send-keys lands in window 0.
Reset-A
Start-Sleep -Milliseconds 1500
Set-Content $f "send-keys -tA:0 'echo m1005k' Enter" -Encoding ASCII
P source-file $f | Out-Null
$in0 = WaitFor { ((P capture-pane -t A:0 -p) -join "`n") -match 'm1005k' } 6000
$in1 = ((P capture-pane -t A:1 -p) -join "`n") -match 'm1005k'
Check ($in0 -and -not $in1) "source-file 'send-keys -tA:0 ...' typed into window 0 only" "send-keys -tA:0: window 0 has it: $in0, active window 1 has it: $in1"
# A command list in one hook line, and a name that looks like a flag.
Reset-A
$f = Join-Path $TMP 'src.conf'
Set-Content $f "rename-window -t:0 L1 ; rename-window -t:2 L2" -Encoding ASCII
P source-file $f | Out-Null
$want = ($script:Base -replace '^0=\S+', '0=L1') -replace '\b2=\S+', '2=L2'
Check (WaitFor { (Names) -eq $want }) "source-file command list with two attached targets" "command list gave '$(Names)', want '$want'"
Reset-A
Set-Content $f "rename-window -t:0 -- -tz" -Encoding ASCII
P source-file $f | Out-Null
Check (WaitFor { (Names) -eq (Want0 '-tz') }) "an operand after -- is the name ('-tz')" "operand case gave '$(Names)'"

# ======================================================================
Write-Head "Raw TCP to the server command handler"
# ======================================================================
foreach ($form in @('-t :0', '-t:0', '-tA:0')) {
    Reset-A
    Send-Tcp 'A' "rename-window $form T"
    $ok = WaitFor { (Names) -eq (Want0 'T') }
    Check $ok "TCP 'rename-window $form T' renamed window 0" "TCP 'rename-window $form T' gave '$(Names)'"
}
foreach ($form in @('-t K2', '-tK2', '-t=K2')) {
    Reset-A
    P kill-session -t K2 | Out-Null
    P new-session -d -s K2 | Out-Null
    Send-Tcp 'A' "kill-session $form"
    $ok = WaitFor { (Sessions) -eq 'A' }
    Check $ok "TCP 'kill-session $form' to session A killed K2 only" "TCP 'kill-session $form' left sessions '$(Sessions)'"
}

# ======================================================================
Write-Head "CLI (already split before the fix; must keep working)"
# ======================================================================
foreach ($form in @(@('-t', ':0'), @('-t:0'), @('-tA:0'))) {
    Reset-A
    P rename-window @form C | Out-Null
    Check ((Names) -eq (Want0 'C')) "CLI 'rename-window $($form -join ' ') C'" "CLI gave '$(Names)'"
}
Reset-A
# Through an array: PowerShell itself splits a bare `-tA:0` argument into
# `-tA:` and `0` for a native program (its -name:value syntax).
$dm = @('display-message', '-tA:0', '-p', '#{window_index}')
$v = ((P @dm) -join '').Trim()
Check ($v -eq '0') "CLI display-message -tA:0 reads window 0" "CLI display-message -tA:0 gave '$v'"
P new-window -d -t A: -nnamed | Out-Null
Check ((Names) -match '\bnamed\b') "CLI new-window -nnamed still names the window" "new-window -nnamed gave '$(Names)'"

P kill-server | Out-Null
Start-Sleep -Milliseconds 800

# ======================================================================
Write-Head "Attached TUI client: bound key and command prompt"
# ======================================================================
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$INJ = Join-Path $TMP "injector.exe"
if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }
if (-not (Test-Path $INJ)) {
    Write-Skip "injector could not be built (csc.exe unavailable)"
} else {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s","A","-x","120","-y","30" -PassThru
    $script:Opened += $p.Id
    $up = WaitFor { ((P display-message -t A -p '#{session_name}') -join '') -eq 'A' } 20000
    Start-Sleep -Milliseconds 2500
    if (-not $up) {
        Write-Fail "TUI: attached session never came up"
    } else {
        P new-window -t A: | Out-Null
        P new-window -t A: | Out-Null
        P select-window -t A:1 | Out-Null
        Start-Sleep -Milliseconds 500
        $script:Base = Names

        P bind-key -n F6 'rename-window -tA:0 B' | Out-Null
        Start-Sleep -Milliseconds 800
        # The first key a fresh client gets can be lost while it settles: retry,
        # renaming is idempotent and a wrong target never turns right.
        $ok = $false
        for ($k = 0; $k -lt 3 -and -not $ok; $k++) {
            & $INJ $p.Id "{F6}" | Out-Null
            $ok = WaitFor { (Names) -ne $script:Base } 2000
        }
        Check (WaitFor { (Names) -eq (Want0 'B') }) "TUI: bound key 'rename-window -tA:0 B' renamed window 0" "TUI: bound key gave '$(Names)'"
        P rename-window -t A:0 zero | Out-Null; $script:Base = Names

        & $INJ $p.Id "^b{SLEEP:300}:{SLEEP:500}rename-window -t:0 Q{ENTER}" | Out-Null
        Check (WaitFor { (Names) -eq (Want0 'Q') }) "TUI: prompt 'rename-window -t:0 Q' renamed window 0" "TUI: prompt gave '$(Names)'"

        # K2 only now: a bind-key without -t goes to the most recent session's
        # server, and psmux keeps bindings per server.
        P new-session -d -s K2 | Out-Null
        Start-Sleep -Milliseconds 500
        & $INJ $p.Id "^b{SLEEP:300}:{SLEEP:500}kill-session -tK2{ENTER}" | Out-Null
        $ok = WaitFor { (Sessions) -eq 'A' }
        Check ($ok -and -not $p.HasExited) "TUI: prompt 'kill-session -tK2' killed K2, client still attached to A" "TUI: prompt kill-session -tK2 left sessions '$(Sessions)', client exited: $($p.HasExited)"
    }
}

P kill-server | Out-Null
Start-Sleep -Milliseconds 800
foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
Remove-Item env:PSMUX_DATA_DIR, env:PSMUX_CONFIG_FILE -EA SilentlyContinue
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

$defaultAfter = ((& $PSMUX ls -F '#{session_name}' 2>&1) -join "`n")
if ($defaultBefore -ne $defaultAfter) {
    Write-Host "  [NOTE] default namespace list changed during the run (this script only uses -L $NS): '$defaultBefore' -> '$defaultAfter'" -ForegroundColor DarkYellow
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:Skipped)"
exit $script:TestsFailed
