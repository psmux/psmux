# Issue #734 follow up: plain `if-shell` (no -F) did not format expand its
# shell command.
#
# Reported by GiorgoLazaridis on the closed #734: on a held empty server
# (`start-server ; set -g exit-empty off`, the state oh-my-claudecode guards
# against), this wrote the literal text `#{pid}` while
# `display-message -p '#{pid}'` printed the server pid:
#
#   if-shell "[IO.File]::WriteAllText('$out','#{pid}'); exit 0" ...
#
# tmux format expands the shell command of if-shell before running it
# (cmd-if-shell.c:86, format_single_from_target), and that of run-shell
# (cmd-run-shell.c:144, format_expand). Both are format_expand, NOT
# format_expand_time, so a `%` is never strftime. On an empty server there is
# no session, but server variables such as #{pid} still resolve.
#
# GROUND TRUTH: the bytes the shell actually received, written to a file by the
# shell command itself, compared with what display-message reports.
#
# Layers: CLI (main.rs, forwards to the server), raw TCP (connection.rs),
# config file via source-file (config.rs, both the line form and the brace
# block form), and an attached TUI client driven by a bound key and by the
# command prompt (WriteConsoleInput through tests\injector.cs).
#
# Isolation: a unique -L namespace only; cleanup is `-L <ns> kill-server` and
# the client window this script opened, by PID.
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

$NS   = "a734_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "s1"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_734_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }
function D($f) { ((P display-message -p $f) -join '').Trim() }
function DT($t, $f) { ((P display-message -t $t -p $f) -join '').Trim() }
function F($name) { Join-Path $TMP $name }
function ReadF($p) { if (Test-Path $p) { ([IO.File]::ReadAllText($p)).Trim() } else { '<missing>' } }
# A pwsh/powershell condition that writes its OWN text, as the shell received
# it, to a file and exits 0. The text stays single quoted for the shell.
function WriteCmd($path, $text) { "[IO.File]::WriteAllText('$path','$text'); exit 0" }

function Kill-Rig {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

$defaultBefore = ((& $PSMUX ls 2>&1) -join "`n")

# ======================================================================
Write-Head "Part A: held EMPTY server (start-server ; exit-empty off)"
# ======================================================================
P -f NUL start-server ';' set -g exit-empty off | Out-Null
Start-Sleep -Milliseconds 500
$spid = D '#{pid}'
Check ($spid -match '^\d+$') "empty server answers #{pid} = $spid" "empty server #{pid} gave '$spid'"
$lsEmpty = ((P ls) -join ' ')
Write-Host "  [INFO] ls on the empty server: '$lsEmpty'" -ForegroundColor DarkCyan

$f = F 'a_if_pid.txt'
$r = ((P if-shell (WriteCmd $f '#{pid}') 'display-message -p A_TRUE' 'display-message -p A_FALSE') -join ' ').Trim()
Check ($r -eq 'A_TRUE') "empty server: if-shell condition ran and took the true branch" "empty server: if-shell result '$r'"
$got = ReadF $f
Check ($got -eq $spid) "empty server: the shell received the expanded #{pid} ($got)" "empty server: shell received '$got', display-message says '$spid'"

$f = F 'a_run_pid.txt'
P run-shell (WriteCmd $f 'R#{pid}') | Out-Null
$got = ReadF $f
Check ($got -eq "R$spid") "empty server: run-shell received the expanded #{pid}" "empty server: run-shell received '$got'"

$r = ((P if-shell -F '#{pid}' 'display-message -p AF_TRUE' 'display-message -p AF_FALSE') -join ' ').Trim()
Check ($r -eq 'AF_TRUE') "empty server: if-shell -F path unchanged" "empty server: if-shell -F gave '$r'"

# ======================================================================
Write-Head "Part B: CLI against a session"
# ======================================================================
P new-session -d -s $SESS | Out-Null
Start-Sleep -Milliseconds 1500
P split-window -t $SESS | Out-Null
Start-Sleep -Milliseconds 1000
$pane2 = DT $SESS '#{pane_id}'
$pane1 = ((P list-panes -t $SESS -F '#{pane_id}') | Select-Object -First 1).Trim()
$pidS  = DT $SESS '#{pid}'

foreach ($case in @(
    @{ n = 'pid';     t = '#{pid}';          want = $pidS },
    @{ n = 'session'; t = '#{session_name}'; want = $SESS },
    @{ n = 'pane';    t = '#{pane_id}';      want = $pane2 },
    @{ n = 'hash';    t = 'a##b';            want = 'a#b' },
    @{ n = 'short';   t = '#S';              want = $SESS }
)) {
    $f = F "b_if_$($case.n).txt"
    $r = ((P if-shell -t $SESS (WriteCmd $f $case.t) 'display-message -p OK' 'display-message -p NO') -join ' ').Trim()
    $got = ReadF $f
    Check ($r -eq 'OK' -and $got -eq $case.want) "if-shell '$($case.t)' -> shell got '$got'" "if-shell '$($case.t)': result '$r', shell got '$got', want '$($case.want)'"
}

# -t picks the pane the formats describe.
$f = F 'b_if_target.txt'
P if-shell -t $pane1 (WriteCmd $f '#{pane_id}') 'display-message -p OK' | Out-Null
$got = ReadF $f
Check ($got -eq $pane1) "if-shell -t $pane1 expands against that pane" "if-shell -t ${pane1}: shell got '$got'"

# A `%` in a shell command is not strftime (tmux uses untimed format_expand).
$f = F 'b_if_pct.txt'
P if-shell -t $SESS (WriteCmd $f '50%d %Y #{session_name}') 'display-message -p OK' | Out-Null
$got = ReadF $f
Check ($got -eq "50%d %Y $SESS") "if-shell keeps % literal: '$got'" "if-shell %: shell got '$got'"
$r = ((P run-shell -t $SESS "Write-Output '50%d %Y #{session_name}'") -join ' ').Trim()
Check ($r -eq "50%d %Y $SESS") "run-shell keeps % literal: '$r'" "run-shell %: got '$r'"

# The expanded condition decides the branch: it expands to `false`.
$r = ((P if-shell -t $SESS '#{?#{session_name},false,true}' 'display-message -p T' 'display-message -p F') -join ' ').Trim()
Check ($r -eq 'F') "if-shell condition expanding to false takes the false branch" "expanded-false condition gave '$r'"
$r = ((P if-shell -t $SESS 'exit 1 # #{pid}' 'display-message -p T' 'display-message -p F') -join ' ').Trim()
Check ($r -eq 'F') "a shell failure after expansion still takes the false branch" "exit 1 condition gave '$r'"

# run-shell: quotes reach the shell intact, ## is #.
$r = ((P run-shell -t $SESS "Write-Output ('x' + '#{session_name}')") -join ' ').Trim()
Check ($r -eq "x$SESS") "run-shell keeps the command's quotes: '$r'" "run-shell quotes mangled: '$r'"
$r = ((P run-shell -t $SESS "Write-Output 'a##b'") -join ' ').Trim()
Check ($r -eq 'a#b') "run-shell turns ## into #" "run-shell ##: got '$r'"
$r = ((P run-shell -t $SESS "Write-Output hi # note #{pid}") -join ' ').Trim()
Check ($r -eq 'hi') "a PowerShell comment still works in run-shell" "run-shell comment: got '$r'"
# A backslash path inside the command survives the trip to the server.
$f = F 'b_run_path.txt'
P run-shell -t $SESS (WriteCmd $f 'P#{session_name}') | Out-Null
Check ((ReadF $f) -eq "P$SESS") "run-shell with a backslash path ran intact" "run-shell path: '$(ReadF $f)'"

# ======================================================================
Write-Head "Part C: raw TCP (server dispatch)"
# ======================================================================
function Send-Tcp($cmd) {
    $port = (Get-Content "$psmuxDir\${NS}__${SESS}.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\${NS}__${SESS}.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $s = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($s); $rd = [System.IO.StreamReader]::new($s)
    $w.Write("AUTH $key`n"); $w.Flush()
    $null = $rd.ReadLine()
    $w.Write("$cmd`n"); $w.Flush()
    $s.ReadTimeout = 15000
    $out = ''
    try { $out = $rd.ReadToEnd() } catch {}
    $tcp.Close()
    return $out.Trim()
}
$f = F 'c_tcp.txt'
$cond = (WriteCmd $f '#{pid}:#{session_name}').Replace('\', '\\')
$r = Send-Tcp "if-shell `"$cond`" `"display-message -p TCP_TRUE`" `"display-message -p TCP_FALSE`""
$got = ReadF $f
Check ($r -match 'TCP_TRUE' -and $got -eq "${pidS}:$SESS") "TCP if-shell: shell got '$got'" "TCP if-shell: reply '$r', shell got '$got'"

# ======================================================================
Write-Head "Part D: config file through source-file"
# ======================================================================
$fc1 = F 'd_conf_line.txt'; $fc2 = F 'd_conf_run.txt'
$conf = F 'test734.conf'
@(
    "if-shell `"$(WriteCmd $fc1 '#{session_name}:#{pid}')`" `"set -g @c734 yes`" `"set -g @c734 no`"",
    "if-shell '#{?#{session_name},false,true}' {",
    "  set -g @b734 yes",
    "}",
    "{",
    "  set -g @b734 no",
    "}",
    "run-shell `"$(WriteCmd $fc2 'R#{session_name}')`""
) | Set-Content -Path $conf -Encoding ASCII
P source-file -t $SESS $conf | Out-Null
Start-Sleep -Milliseconds 2500
$got = ReadF $fc1
Check ($got -eq "${SESS}:$pidS") "config if-shell: shell got '$got'" "config if-shell: shell got '$got'"
Check (((P show -gv '@c734') -join '').Trim() -eq 'yes') "config if-shell ran the true branch" "config if-shell @c734 = '$((P show -gv '@c734') -join '')'"
$b = ((P show -gv '@b734') -join '').Trim()
Check ($b -eq 'no') "config brace if-shell: expanded condition picked the else block" "config brace if-shell @b734 = '$b'"
$got = ReadF $fc2
Check ($got -eq "R$SESS") "config run-shell expanded its command" "config run-shell: '$got'"

Kill-Rig

# ======================================================================
Write-Head "Part E: attached TUI client (bound key + command prompt)"
# ======================================================================
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$INJ = Join-Path $TMP "injector.exe"
if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }

$p = Start-Process -FilePath $PSMUX -ArgumentList "-f","NUL","-L",$NS,"new-session","-s",$SESS,"-x","120","-y","30" -PassThru
$script:Opened += $p.Id
$up = $false
for ($i = 0; $i -lt 80; $i++) {
    Start-Sleep -Milliseconds 250
    if ((DT $SESS '#{session_name}') -eq $SESS) { $up = $true; break }
}
Start-Sleep -Milliseconds 2500
if (-not $up) {
    Write-Fail "TUI: attached session never came up"
} else {
    $tpid = DT $SESS '#{pid}'
    $f = F 'e_tui_cli.txt'
    P if-shell -t $SESS (WriteCmd $f '#{pid}') 'display-message OK' | Out-Null
    Check ((ReadF $f) -eq $tpid) "TUI session: CLI if-shell expanded #{pid}" "TUI session: CLI if-shell got '$(ReadF $f)'"

    if (-not (Test-Path $INJ)) {
        Write-Skip "injector could not be built (csc.exe unavailable)"
    } else {
        # Bound key: the condition expands to 0 and must pick the false branch.
        # Unexpanded, pwsh read the `#` as a comment, exited 0, and chose true.
        P bind-key Y if-shell '#{==:#{pane_id},nomatch}' 'set -g @k734 yes' 'set -g @k734 no' | Out-Null
        & $INJ $p.Id "^b{SLEEP:300}Y" | Out-Null
        Start-Sleep -Milliseconds 2500
        $k = ((P show -gv '@k734') -join '').Trim()
        Check ($k -eq 'no') "TUI: bound key if-shell expanded its condition (false branch)" "TUI: bound key gave @k734 = '$k'"

        # The same binding made in a config file (the usual place for it).
        $bconf = F 'e_bind.conf'
        "bind-key Z if-shell '#{==:#{pane_id},nomatch}' 'set -g @z734 yes' 'set -g @z734 no'" |
            Set-Content -Path $bconf -Encoding ASCII
        P source-file -t $SESS $bconf | Out-Null
        & $INJ $p.Id "^b{SLEEP:300}Z" | Out-Null
        Start-Sleep -Milliseconds 2500
        $z = ((P show -gv '@z734') -join '').Trim()
        Check ($z -eq 'no') "TUI: config bound key if-shell expanded its condition (false branch)" "TUI: config bound key gave @z734 = '$z'"

        # Command prompt: same condition typed at prefix + :
        & $INJ $p.Id "^b{SLEEP:300}:{SLEEP:500}if-shell '#{LBRACE}==:#{LBRACE}session_name{RBRACE},nomatch{RBRACE}' 'set -g @p734 yes' 'set -g @p734 no'{ENTER}" | Out-Null
        Start-Sleep -Milliseconds 2500
        $pp = ((P show -gv '@p734') -join '').Trim()
        Check ($pp -eq 'no') "TUI: command prompt if-shell expanded its condition (false branch)" "TUI: command prompt gave @p734 = '$pp'"
    }
    $alive = (DT $SESS '#{session_windows}') -eq '1'
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
