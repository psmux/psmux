# Issue #774: session names are case-sensitive in tmux, psmux matched them
# case-insensitively, even with the exact `=name` form.
#
# A session is located by opening `<data dir>\<name>.port`, and that directory
# is case-insensitive on Windows. So `has-session -t =REPRO-CASE` answered for
# `repro-case`, `display-message -t =REPRO-CASE` printed `repro-case`, and
# `kill-session -t =REPRO-CASE` KILLED `repro-case`. tmux compares names with
# strcmp (session.c session_find; the prefix and fnmatch fallbacks in
# cmd-find.c cmd_find_get_session are case-sensitive too) and answers
# `can't find session: REPRO-CASE`.
#
# psmux cannot keep `repro-case` and `REPRO-CASE` as two sessions (registry
# files, the single-server mutex and the warm claim all live in
# case-insensitive namespaces), so `new-session -s REPRO-CASE` stays refused,
# now with a message that names the session in the way. rename-session had no
# duplicate check at all and orphaned the session it wrote over; it now
# refuses like tmux (`duplicate session: NAME`).
#
# Layers: CLI (-L namespace), raw TCP to a server (default namespace of a
# private data dir), and an attached TUI client (attach gate, and the `:`
# command prompt driven through tests\injector.cs).
#
# Isolation: a private PSMUX_DATA_DIR for the whole run, so even the default
# namespace used by the TCP part is not the user's. Cleanup is
# `-L <ns> kill-server` and `kill-session -t` of sessions this suite created;
# nothing is killed by name.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:PSMUX_TARGET_SESSION = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_774_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = Join-Path $TMP "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$psmuxDir = $env:PSMUX_DATA_DIR
$env:PSMUX_NO_WARM = "1"

$NS = "a774-" + [guid]::NewGuid().ToString('N').Substring(0, 6)

# Runs psmux in the test namespace, returns @{ Out; Rc }.
function P {
    $o = & $PSMUX -L $NS @args 2>&1
    return @{ Out = (($o | ForEach-Object { "$_" }) -join "`n").Trim(); Rc = $LASTEXITCODE }
}
# Same, default namespace of the private data dir.
function P0 {
    $o = & $PSMUX @args 2>&1
    return @{ Out = (($o | ForEach-Object { "$_" }) -join "`n").Trim(); Rc = $LASTEXITCODE }
}
function Names { @((P list-sessions -F '#{session_name}').Out -split "`n" | Where-Object { $_ }) }
function Names0 { @((P0 list-sessions -F '#{session_name}').Out -split "`n" | Where-Object { $_ }) }

function Stop-Opened {
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
}

function Cleanup {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    foreach ($s in "alpha", "beta", "gamma") { & $PSMUX kill-session -t "=$s" 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 800
    Stop-Opened
}

# ── Part 1: the reporter's steps on the CLI ─────────────────────────────────
Write-Head "CLI: exact and plain targets do not fold case"
$r = P new-session -d -s repro-case
Check ($r.Rc -eq 0) "new-session -s repro-case" "new-session -s repro-case failed: rc=$($r.Rc) $($r.Out)"

$r = P has-session -t =repro-case
Check ($r.Rc -eq 0) "has-session -t =repro-case (positive control)" "positive control failed: rc=$($r.Rc) $($r.Out)"

$r = P has-session -t =REPRO-CASE
Check ($r.Rc -eq 1 -and $r.Out -eq "psmux: can't find session: REPRO-CASE") `
    "has-session -t =REPRO-CASE: can't find session, rc 1" `
    "has-session -t =REPRO-CASE answered for repro-case: rc=$($r.Rc) out='$($r.Out)'"

$r = P has-session -t REPRO-CASE
Check ($r.Rc -eq 1 -and $r.Out -match "can't find session: REPRO-CASE") `
    "has-session -t REPRO-CASE (no =): can't find session, rc 1" `
    "plain REPRO-CASE folded onto repro-case: rc=$($r.Rc) out='$($r.Out)'"

$r = P display-message -p -t =REPRO-CASE '#{session_name}'
Check ($r.Rc -eq 1 -and $r.Out -notmatch "^repro-case$") `
    "display-message -t =REPRO-CASE refused" `
    "display-message -t =REPRO-CASE printed '$($r.Out)' rc=$($r.Rc)"

$r = P list-windows -t REPRO-CASE -F '#{window_index}'
Check ($r.Rc -eq 1) "list-windows -t REPRO-CASE refused" "list-windows -t REPRO-CASE rc=$($r.Rc) out='$($r.Out)'"

$r = P send-keys -t =REPRO-CASE C-u
Check ($r.Rc -eq 1) "send-keys -t =REPRO-CASE refused" "send-keys -t =REPRO-CASE rc=$($r.Rc) out='$($r.Out)'"

$r = P display-message -p -t =repro-case:0 '#{session_name}'
Check ($r.Rc -eq 0 -and $r.Out -eq "repro-case") "exact spelling with a window part still resolves" "=repro-case:0 rc=$($r.Rc) out='$($r.Out)'"

Write-Head "CLI: a case-variant new-session is refused and names the existing session"
$r = P new-session -d -s REPRO-CASE
Check ($r.Rc -eq 1 -and $r.Out -match "^duplicate session: REPRO-CASE" -and $r.Out -match "repro-case") `
    "new-session -s REPRO-CASE: duplicate session naming repro-case" `
    "new-session -s REPRO-CASE: rc=$($r.Rc) out='$($r.Out)'"
$r = P new-session -d -A -s REPRO-CASE
Check ($r.Rc -eq 1 -and $r.Out -match "^duplicate session: REPRO-CASE") `
    "new-session -A -s REPRO-CASE does not attach to repro-case" `
    "new-session -A -s REPRO-CASE: rc=$($r.Rc) out='$($r.Out)'"
$n = Names
Check (($n -join ',') -ceq "repro-case") "list-sessions shows only repro-case" "list-sessions: $($n -join ',')"

Write-Head "CLI: kill-session -t =REPRO-CASE does not kill repro-case"
$r = P kill-session -t =REPRO-CASE
Check ($r.Rc -eq 1 -and $r.Out -match "can't find session: REPRO-CASE") `
    "kill-session -t =REPRO-CASE: can't find session, rc 1" `
    "kill-session -t =REPRO-CASE: rc=$($r.Rc) out='$($r.Out)'"
$r = P has-session -t =repro-case
Check ($r.Rc -eq 0) "repro-case survived the kill aimed at REPRO-CASE" "repro-case is gone (rc=$($r.Rc))"

Write-Head "CLI: mixed-case names match exactly"
$r = P new-session -d -s Work
Check ($r.Rc -eq 0) "new-session -s Work" "new-session -s Work rc=$($r.Rc) $($r.Out)"
Check ((P has-session -t =Work).Rc -eq 0) "has-session -t =Work" "has-session -t =Work failed"
$r = P has-session -t =work
Check ($r.Rc -eq 1 -and $r.Out -match "can't find session: work") "has-session -t =work refused" "has-session -t =work rc=$($r.Rc) out='$($r.Out)'"
$sid = (P display-message -p -t =Work '#{session_id}').Out
$r = P display-message -p -t $sid '#{session_name}'
Check ($r.Out -ceq "Work") "session id target $sid resolves to Work" "id target: rc=$($r.Rc) out='$($r.Out)'"

Write-Head "CLI: rename-session"
$r = P rename-session -t =Work WoRk
Check ($r.Rc -eq 0) "rename to another spelling of its own name is allowed (tmux)" "case-only self rename rc=$($r.Rc) out='$($r.Out)'"
Check ((P has-session -t =WoRk).Rc -eq 0 -and (P has-session -t =Work).Rc -eq 1) "renamed session answers only to WoRk" "case-only rename did not take"
$r = P rename-session -t =WoRk REPRO-CASE
Check ($r.Rc -eq 1 -and $r.Out -eq "duplicate session: REPRO-CASE") `
    "rename onto a case variant of a live session: duplicate session" `
    "rename onto REPRO-CASE: rc=$($r.Rc) out='$($r.Out)'"
$r = P rename-session -t =WoRk repro-case
Check ($r.Rc -eq 1 -and $r.Out -eq "duplicate session: repro-case") `
    "rename onto an exact live name: duplicate session" `
    "rename onto repro-case: rc=$($r.Rc) out='$($r.Out)'"
$n = Names
Check ((($n | Sort-Object) -join ',') -ceq "repro-case,WoRk") "both sessions still listed after the refused renames" "list-sessions: $($n -join ',')"
$r = P display-message -p -t =repro-case '#{session_name}'
Check ($r.Out -ceq "repro-case") "repro-case still reachable (registry not overwritten)" "repro-case: rc=$($r.Rc) out='$($r.Out)'"

$r = P kill-session -t =repro-case
Check ($r.Rc -eq 0) "kill-session -t =repro-case (exact) still kills" "exact kill rc=$($r.Rc) out='$($r.Out)'"
Check ((P has-session -t =repro-case).Rc -eq 1) "repro-case gone after the exact kill" "repro-case still there"

# The reporter ran without -L. There the folded lookup reaches the session's
# own server and the kill really lands (under -L it timed out instead).
Write-Head "CLI default namespace: kill-session -t =GAMMA does not kill gamma"
$r = P0 new-session -d -s gamma
Check ($r.Rc -eq 0) "new-session -s gamma" "new-session -s gamma rc=$($r.Rc) $($r.Out)"
$r = P0 kill-session -t =GAMMA
Check ($r.Rc -eq 1 -and $r.Out -eq "psmux: can't find session: GAMMA") `
    "kill-session -t =GAMMA: can't find session, rc 1" "kill-session -t =GAMMA: rc=$($r.Rc) out='$($r.Out)'"
Check ((P0 has-session -t =gamma).Rc -eq 0) "gamma survived" "gamma was killed by kill-session -t =GAMMA"
$null = P0 kill-session -t =gamma

# ── Part 2: raw TCP to a server ─────────────────────────────────────────────
Write-Head "TCP: kill-session -t BETA sent to another server does not kill beta"
$r0 = P0 new-session -d -s alpha
$r1 = P0 new-session -d -s beta
if ($r0.Rc -ne 0 -or $r1.Rc -ne 0) {
    Write-Fail "could not create alpha/beta: $($r0.Out) $($r1.Out)"
} else {
    function Send-Tcp($session, $line) {
        $port = [int](Get-Content (Join-Path $psmuxDir "$session.port") -Raw).Trim()
        $key = (Get-Content (Join-Path $psmuxDir "$session.key") -Raw).Trim()
        $c = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $port)
        $s = $c.GetStream(); $s.ReadTimeout = 3000
        $w = [System.IO.StreamWriter]::new($s); $w.NewLine = "`n"
        $w.WriteLine("AUTH $key"); $w.WriteLine($line); $w.Flush()
        $c.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $rd = [System.IO.StreamReader]::new($s)
        try { $txt = $rd.ReadToEnd() } catch { $txt = "" }
        $c.Close()
        return $txt
    }
    $null = Send-Tcp "alpha" "kill-session -t BETA"
    Start-Sleep -Milliseconds 1200
    Check ((P0 has-session -t =beta).Rc -eq 0) "beta survived kill-session -t BETA over TCP" "kill-session -t BETA over TCP killed beta"
    Check ((P0 has-session -t =alpha).Rc -eq 0) "alpha (the receiving server) survived too" "alpha died on kill-session -t BETA"
    $null = Send-Tcp "alpha" "kill-session -t beta"
    Start-Sleep -Milliseconds 1200
    Check ((P0 has-session -t =beta).Rc -eq 1) "kill-session -t beta over TCP (exact) kills beta" "exact TCP kill left beta alive"
    $n = Names0
    Check (($n -join ',') -ceq "alpha") "only alpha left" "default namespace: $($n -join ',')"
}

# ── Part 3: attached TUI client ─────────────────────────────────────────────
Write-Head "TUI: attach gate and the : command prompt"
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$INJ = Join-Path $TMP "injector.exe"
if ($csc -and (Test-Path $csc)) {
    & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}
$CONF = Join-Path $TMP "a774.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@("set -g default-shell $POWERSHELL", "set -g status-left ''") | Set-Content -Path $CONF -Encoding ASCII

$r = P new-session -d -s victim
Check ($r.Rc -eq 0) "new-session -s victim" "new-session -s victim rc=$($r.Rc) $($r.Out)"
$proc = Start-Process -FilePath $PSMUX -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s","Tui","-x","100","-y","30" -PassThru
$script:Opened += $proc.Id
$up = $false
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 250
    if ((P has-session -t =Tui).Rc -eq 0) { $up = $true; break }
}
if (-not $up) {
    Write-Fail "attached session Tui never came up"
} else {
    Start-Sleep -Milliseconds 2000
    $clients = (P list-clients -F '#{client_session}').Out
    Check ($clients -match "Tui") "attached client is on Tui" "list-clients: '$clients'"

    # A second client asking for TUI (wrong case) must be refused at the gate.
    $r = P attach-session -t TUI
    Check ($r.Rc -eq 1 -and $r.Out -match "can't find session: TUI") `
        "attach -t TUI: can't find session, rc 1" "attach -t TUI: rc=$($r.Rc) out='$($r.Out)'"

    if (-not (Test-Path $INJ)) {
        Write-Skip "injector.exe could not be built; prompt checks skipped"
    } else {
        & $INJ $proc.Id "^b{SLEEP:400}:kill-session -t VICTIM{ENTER}" | Out-Null
        Start-Sleep -Milliseconds 1500
        Check ((P has-session -t =victim).Rc -eq 0) "prompt kill-session -t VICTIM leaves victim alive" "prompt kill-session -t VICTIM killed victim"
        Check ((P has-session -t =Tui).Rc -eq 0) "prompt kill-session -t VICTIM leaves Tui alive" "Tui died"

        & $INJ $proc.Id "^b{SLEEP:400}:switch-client -t VICTIM{ENTER}" | Out-Null
        Start-Sleep -Milliseconds 1500
        $clients = (P list-clients -F '#{client_session}').Out
        Check ($clients -match "Tui" -and $clients -notmatch "victim") "prompt switch-client -t VICTIM does not switch" "list-clients after VICTIM switch: '$clients'"

        & $INJ $proc.Id "^b{SLEEP:400}:switch-client -t victim{ENTER}" | Out-Null
        Start-Sleep -Milliseconds 2500
        $clients = (P list-clients -F '#{client_session}').Out
        Check ($clients -match "victim") "prompt switch-client -t victim (exact) switches" "list-clients after victim switch: '$clients'"
    }
}

Cleanup
Start-Sleep -Milliseconds 500
# Report (never kill by name) anything of this run's namespace still alive.
$left = @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
    Where-Object { $_.CommandLine -match [regex]::Escape($NS) })
foreach ($p in $left) { Write-Info "leftover pid $($p.ProcessId): $($p.CommandLine)" }
if ($savedDataDir) { $env:PSMUX_DATA_DIR = $savedDataDir } else { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue }
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed  skipped: $script:Skipped"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
