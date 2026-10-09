# Issue #734 side findings: three parity defects found while fixing #734.
#
# 1. A held empty server (`start-server ; set -g exit-empty off`, no session)
#    answered `display-message -p '#{session_name}'` with `__warm__`, the warm
#    pool's internal name, and #I/#W/#{pane_id} described its spare window.
#    tmux has no session there (CMD_FIND_CANFAIL): those formats are empty and
#    #{server_sessions} is 0. The next new-session must still claim that same
#    server (the #734 promise), so the claim is checked too.
# 2. A config file if-shell with the else block on the closing brace's line
#    (`} {`) or a whole block on one line (`if-shell C { A } { B }`) was not
#    understood: tmux's lexer reads braces as tokens wherever they stand.
# 3. `bind-key -N 'note' -T mytbl x ...` bound nothing: no bind-key parser knew
#    -N, and psmux kept no notes for `list-keys -N` (tmux cmd-bind-key.c,
#    cmd-list-keys.c).
#
# Layers: CLI (main.rs), raw TCP (server/connection.rs), config files
# (source-file and -f at startup, config.rs), and an attached TUI client driven
# by WriteConsoleInput (tests\injector.cs): command prompt and bound keys.
#
# Isolation: unique -L namespaces only; cleanup is `-L <ns> kill-server` and
# the client window this script opened, by PID. Never a by-name kill.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null
$env:PSMUX_CONFIG_FILE  = $null

$TAG  = [guid]::NewGuid().ToString('N').Substring(0, 6)
$NS   = "aside_" + $TAG
$SESS = "s1"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_734side_" + $TAG)
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }
function J { (($args | ForEach-Object { $_ }) -join "`n").Trim() }
function D($f) { ((P display-message -p $f) -join '').Trim() }
function DT($t, $f) { ((P display-message -t $t -p $f) -join '').Trim() }
function Opt($name) { ((P show-options -gqv $name) -join '').Trim() }
function F($name) { Join-Path $TMP $name }

function Send-Tcp($base, $cmd) {
    $port = (Get-Content "$psmuxDir\$base.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\$base.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $s = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($s); $rd = [System.IO.StreamReader]::new($s)
    $w.Write("AUTH $key`n"); $w.Flush()
    $null = $rd.ReadLine()
    $w.Write("$cmd`n"); $w.Flush()
    $tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
    $s.ReadTimeout = 10000
    $out = ''
    try { $out = $rd.ReadToEnd() } catch {}
    $tcp.Close()
    return $out.TrimEnd("`r", "`n")
}

function Kill-Rig {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

$defaultBefore = ((& $PSMUX ls 2>&1) -join "`n")

# ======================================================================
Write-Head "1A: held empty server, CLI"
# ======================================================================
P -f NUL start-server ';' set -g exit-empty off | Out-Null
Start-Sleep -Milliseconds 800
$spid = D '#{pid}'
Check ($spid -match '^\d+$') "held server answers #{pid} = $spid" "held server #{pid} gave '$spid'"
$fmt = D '[#{session_name}][#S][#{session_id}][#{window_index}][#I][#W][#{pane_id}][#{session_windows}]'
Check ($fmt -eq '[][][][][][][][]') "no session: session, window and pane formats are empty" "no session formats gave '$fmt'"
Check ($fmt -notmatch 'warm') "the internal name never shows" "internal name leaked: '$fmt'"
$cnt = D '#{server_sessions}'
Check ($cnt -eq '0') "#{server_sessions} is 0" "#{server_sessions} gave '$cnt'"
$loops = D '[#{S:#{session_name}}][#{W:#{window_name}}][#{P:#{pane_id}}]'
Check ($loops -eq '[][][]') "S: W: P: loops walk nothing" "loops gave '$loops'"
$ls = P list-sessions; $lsRc = $LASTEXITCODE
Check ($lsRc -eq 0 -and (J $ls) -eq '') "list-sessions prints nothing, exit 0" "list-sessions rc=$lsRc out='$(J $ls)'"
$lsf = J (P list-sessions -F '#{session_name}')
Check ($lsf -eq '') "list-sessions -F prints nothing" "list-sessions -F gave '$lsf'"
$lw = J (P list-windows -a)
Check ($lw -eq '') "list-windows -a prints nothing" "list-windows -a gave '$lw'"
P has-session -t __warm__ | Out-Null; $hrc = $LASTEXITCODE
Check ($hrc -ne 0) "has-session -t __warm__ finds no session" "has-session -t __warm__ rc=$hrc"

# ======================================================================
Write-Head "1B: held empty server, raw TCP"
# ======================================================================
$warmBase = "${NS}____warm__"
if (Test-Path "$psmuxDir\$warmBase.port") {
    $r = Send-Tcp $warmBase "display-message -p '[#{session_name}][#S][#{window_name}]'"
    Check ($r -eq '[][][]') "TCP display-message on the held server: '$r'" "TCP display-message gave '$r'"
    $si = Send-Tcp $warmBase "session-info"
    Check ($si -like '__warm__:*') "session-info (internal, not a format) still identifies the standby" "session-info gave '$si'"
} else {
    Write-Fail "held standby registry file $warmBase.port missing"
}

# ======================================================================
Write-Head "1C: the next new-session claims that same server"
# ======================================================================
P new-session -d -s $SESS | Out-Null
Start-Sleep -Milliseconds 800
$cpid = DT $SESS '#{pid}'
Check ($cpid -eq $spid) "claim landed in the held process ($cpid)" "claim pid $cpid, held pid $spid"
$named = DT $SESS '#{session_name}|#S|#{session_windows}'
Check ($named -eq "$SESS|$SESS|1") "the claimed session formats its name: '$named'" "claimed session formats '$named'"
$untargeted = D '#{session_name}'
Check ($untargeted -eq $SESS) "untargeted display-message now sees the session" "untargeted gave '$untargeted'"
$lsf = J (P list-sessions -F '#{session_name}')
Check ($lsf -eq $SESS) "list-sessions -F lists only the claimed session" "list-sessions -F gave '$lsf'"
Kill-Rig

# ======================================================================
Write-Head "2A: if-shell braces through source-file"
# ======================================================================
$conf2 = F 'braces.conf'
@(
    "if-shell 'exit 0' { set -g @a yes } { set -g @a no }",
    "if-shell 'exit 1' { set -g @b yes } { set -g @b no }",
    "if-shell 'exit 1' {",
    "  set -g @c yes",
    "} {",
    "  set -g @c no",
    "}",
    "if-shell 'exit 0' {",
    "  set -g @d yes",
    "} {",
    "  set -g @d no",
    "}",
    "if-shell 'exit 1' {",
    "  set -g @e yes",
    "}",
    "{",
    "  set -g @e no",
    "}",
    "if-shell -F '#{==:x,x}' { set -g @f '{q}' }",
    "set -g @after reached"
) | Set-Content -Path $conf2 -Encoding ASCII
$want = [ordered]@{ '@a' = 'yes'; '@b' = 'no'; '@c' = 'no'; '@d' = 'yes'; '@e' = 'no'; '@f' = '{q}'; '@after' = 'reached' }

P -f NUL new-session -d -s $SESS | Out-Null
Start-Sleep -Milliseconds 800
$warn = J (P source-file $conf2)
Check ($warn -eq '') "source-file reports no error" "source-file said '$warn'"
foreach ($k in $want.Keys) {
    $v = Opt $k
    Check ($v -eq $want[$k]) "source-file: $k = $v" "source-file: $k = '$v', want '$($want[$k])'"
}

# ======================================================================
Write-Head "2B: the same file over raw TCP"
# ======================================================================
foreach ($k in $want.Keys) { P set -gu $k | Out-Null }
$null = Send-Tcp "${NS}__$SESS" "source-file '$conf2'"
Start-Sleep -Milliseconds 800
foreach ($k in $want.Keys) {
    $v = Opt $k
    Check ($v -eq $want[$k]) "TCP source-file: $k = $v" "TCP source-file: $k = '$v', want '$($want[$k])'"
}
Kill-Rig

# ======================================================================
Write-Head "2C: the same file as the startup config (-f)"
# ======================================================================
$out = J (P -f $conf2 new-session -d -s $SESS)
Check ($out -notmatch 'unknown command') "startup load reports no unknown command" "startup load said '$out'"
Start-Sleep -Milliseconds 500
foreach ($k in $want.Keys) {
    $v = Opt $k
    Check ($v -eq $want[$k]) "startup config: $k = $v" "startup config: $k = '$v', want '$($want[$k])'"
}
Kill-Rig

# ======================================================================
Write-Head "3A: bind-key -N from the CLI"
# ======================================================================
P -f NUL new-session -d -s $SESS | Out-Null
Start-Sleep -Milliseconds 800
$prefix = ((P show-options -gv prefix) -join '').Trim()
P bind-key -N 'note' -T mytbl x display-message hi | Out-Null
P bind-key -T mytbl y display-message yo | Out-Null
P bind-key -N 'two word note' z display-message zed | Out-Null
$lk = (P list-keys -T mytbl) | ForEach-Object { ($_ -replace '\s+', ' ').Trim() }
Check ($lk -contains 'bind-key -T mytbl x display-message hi') "list-keys -T mytbl shows the noted binding" "list-keys -T mytbl: '$($lk -join ' | ')'"
Check ($lk -contains 'bind-key -T mytbl y display-message yo') "list-keys -T mytbl shows the plain binding" "list-keys -T mytbl: '$($lk -join ' | ')'"
$nt = J (P list-keys -N -T mytbl)
Check ($nt -eq "$prefix x note") "list-keys -N -T mytbl: '$nt'" "list-keys -N -T mytbl gave '$nt'"
$na = (P list-keys -N -a -T mytbl) | ForEach-Object { ($_ -replace '\s+', ' ').Trim() }
Check (($na -contains "$prefix x note") -and ($na -contains "$prefix y display-message yo")) "list-keys -N -a lists the command where there is no note" "list-keys -N -a gave '$($na -join ' | ')'"
$np = J (P list-keys -N)
Check ($np -eq "$prefix z two word note") "list-keys -N (prefix and root): '$np'" "list-keys -N gave '$np'"
$bad = J (P list-keys -T prefix note)
Check ($bad -eq '') "the note was not bound as a key" "a key named note exists: '$bad'"
P bind-key -T mytbl x display-message again | Out-Null
$nt2 = J (P list-keys -N -T mytbl)
Check ($nt2 -eq '') "rebinding without -N drops the note, like tmux" "after rebind list-keys -N gave '$nt2'"

# ======================================================================
Write-Head "3B: bind-key -N over raw TCP and from a config file"
# ======================================================================
$null = Send-Tcp "${NS}__$SESS" "bind-key -N 'tcp note' -T tcptbl q display-message tq"
$tq = J (P list-keys -N -T tcptbl)
Check ($tq -eq "$prefix q tcp note") "TCP bind-key -N: '$tq'" "TCP bind-key -N gave '$tq'"
$tl = J (Send-Tcp "${NS}__$SESS" "list-keys -N -T tcptbl")
Check ($tl -eq "$prefix q tcp note") "TCP list-keys -N: '$tl'" "TCP list-keys -N gave '$tl'"
$conf3 = F 'notes.conf'
@(
    "bind-key -N `"config note`" -T cfgtbl w display-message cw",
    "bind -nrN 'root repeat' F8 next-window"
) | Set-Content -Path $conf3 -Encoding ASCII
P source-file $conf3 | Out-Null
$cw = J (P list-keys -N -T cfgtbl)
Check ($cw -eq "$prefix w config note") "config bind-key -N: '$cw'" "config bind-key -N gave '$cw'"
$f8 = J (P list-keys -T root F8)
Check ($f8 -match '^bind-key -r -T root F8 next-window$') "config bind -nrN keeps -r and the root table: '$f8'" "config bind -nrN gave '$f8'"
Kill-Rig

# ======================================================================
Write-Head "TUI: attached client on a held server (command prompt, bound keys)"
# ======================================================================
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$INJ = Join-Path $TMP "injector.exe"
if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }

P -f NUL start-server ';' set -g exit-empty off | Out-Null
Start-Sleep -Milliseconds 800
$hpid = D '#{pid}'
$p = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"new-session","-s",$SESS,"-x","120","-y","30" -PassThru
$script:Opened += $p.Id
$up = $false
for ($i = 0; $i -lt 80; $i++) {
    Start-Sleep -Milliseconds 250
    if ((DT $SESS '#{session_name}') -eq $SESS) { $up = $true; break }
}
Start-Sleep -Milliseconds 2000
if (-not $up) {
    Write-Fail "TUI: attached session never came up"
} else {
    Check ((DT $SESS '#{pid}') -eq $hpid) "TUI: the attached new-session claimed the held server" "TUI: claimed pid $(DT $SESS '#{pid}') vs held $hpid"
    Check ((DT $SESS '#{client_session}') -eq $SESS) "TUI: client_session names the session" "TUI: client_session '$(DT $SESS '#{client_session}')'"
    if (-not (Test-Path $INJ)) {
        Write-Skip "injector could not be built (csc.exe unavailable)"
    } else {
        # Item 1: a format run from the attached client sees its session.
        # (Braces are typed as {LBRACE}/{RBRACE}: the injector reads {..} as a token.)
        & $INJ $p.Id "^b{SLEEP:300}:{SLEEP:500}set -gF @tui_s '#S:#{LBRACE}session_windows{RBRACE}'{ENTER}" | Out-Null
        Start-Sleep -Milliseconds 1500
        $ts = Opt '@tui_s'
        Check ($ts -eq "${SESS}:1") "TUI command prompt: set -gF '#S:#{session_windows}' gave '$ts'" "TUI command prompt: @tui_s = '$ts'"

        # Item 2: source-file of the brace config from the command prompt.
        & $INJ $p.Id "^b{SLEEP:300}:{SLEEP:500}source-file $conf2{ENTER}" | Out-Null
        Start-Sleep -Milliseconds 3000
        $ok2 = $true
        foreach ($k in $want.Keys) { if ((Opt $k) -ne $want[$k]) { $ok2 = $false; Write-Host "    $k = '$(Opt $k)'" } }
        Check $ok2 "TUI command prompt: source-file ran every brace block right" "TUI command prompt: brace config options wrong"

        # Item 3: noted bindings actually fire from the keyboard, in the root
        # and prefix tables. ('@x734' quoted: a bare @name is PowerShell
        # splatting.) A key in a `switch-client -T` table does not fire for
        # injected input on the unfixed binary either, unrelated to -N, so
        # that path is not used here.
        P bind-key -n -N 'root mark' F7 set -g '@r734' pressed | Out-Null
        P bind-key -N 'prefix mark' X set -g '@x734' pressed | Out-Null
        & $INJ $p.Id "{F7}" | Out-Null
        Start-Sleep -Milliseconds 1200
        & $INJ $p.Id "^b{SLEEP:300}X" | Out-Null
        Start-Sleep -Milliseconds 1500
        $r = Opt '@r734'; $x = Opt '@x734'
        Check ($r -eq 'pressed') "TUI: F7 ran the noted root binding" "TUI: @r734 = '$r'"
        Check ($x -eq 'pressed') "TUI: prefix X ran the noted prefix binding" "TUI: @x734 = '$x'"
        $pfx = ((P show-options -gv prefix) -join '').Trim()
        $rn = (P list-keys -N) | ForEach-Object { ($_ -replace '\s+', ' ').Trim() }
        Check (($rn -contains "$pfx X prefix mark") -and ($rn -contains "$pfx F7 root mark")) "TUI: list-keys -N lists both notes" "TUI: list-keys -N gave '$($rn -join ' | ')'"
    }
    $alive = (DT $SESS '#{session_name}') -eq $SESS
    Check $alive "TUI: session still answers after the key paths" "TUI: session did not answer"
}

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

$defaultAfter = ((& $PSMUX ls 2>&1) -join "`n")
Check ($defaultBefore -eq $defaultAfter) "default namespace session list unchanged" "default namespace changed: before '$defaultBefore' after '$defaultAfter'"

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:Skipped)"
exit $script:TestsFailed
