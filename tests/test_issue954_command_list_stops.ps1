# Issue #954: a command list goes on after a failing command.
#
# tmux gives every parsed command list a group (cmd.c:602 cmd_list_new, each
# line of input its own, cmd-parse.y:895-904) and, when a command returns
# CMD_RETURN_ERROR, removes the items after it that share the group
# (cmd-queue.c:750-754, cmdq_remove_group at :443-456). An if-shell branch is
# a list of its own inserted after the if-shell (cmd-if-shell.c:99-100), so a
# failure inside it drops only the rest of the branch. A config or sourced
# file gives each line its own group, so the NEXT line still runs.
#
# psmux went on after the failure at the command prompt and in a key binding
# (the attached client sent a list one command per line and the server never
# cleared a persistent connection's queue), and on the CLI after show-options
# (its error was written without being marked as one).
#
# Routes: the CLI, a raw PERSISTENT connection (the line as the client sends
# it), and a real attached client driven by tests\injector.cs
# (WriteConsoleInput) for prefix + : and for a key binding.
#
# Isolation: private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace
# per server, cleanup `-L <ns> kill-server` plus the client PID this suite
# started. PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $ok, $detail) { if ($ok) { Write-Pass $name } else { Write-Fail "$name $detail" } }

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cls_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
$sourced = Join-Path $TempHome "sourced.conf"
@'
set -g @sa 1 ; show-options @nope ; set -g @sb 1
set -g @sc 1
'@ | Set-Content -Path $sourced
@"
set -g display-time 20000
bind -n F9 set -g @ka 1 \; show-options @nope \; set -g @kb 1
bind -n F10 set -g @ok1 1 \; set -g @ok2 1 \; set -g @ok3 1
bind -n F11 run-shell 'exit 0' \; set -g @rs 1
set -g @fa 1 ; show-options @nope ; set -g @fb 1
set -g @fc 1
"@ | Set-Content -Path $env:PSMUX_CONFIG_FILE
$script:NS = ""
$S = "cls"
function P { & $PSMUX -L $script:NS @args 2>&1 }
# The value of a global user option, or '' when it is not set (show -gv of
# an unset @name fails with "invalid option").
function Opt($name) {
    $v = (P show -gv $name) -join ''
    if ($LASTEXITCODE -ne 0) { return '' }
    return $v.Trim()
}
function New-Server {
    $script:NS = "cls_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1500
}
function Stop-Server { P kill-server | Out-Null; Start-Sleep -Milliseconds 300 }

Write-Host "`n=== a command list stops at its first failing command ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    # ---------------------------------------------------------------- CLI
    Write-Host "`n[1] CLI: set -g @a 1 ; K ; set -g @b 1" -ForegroundColor Yellow
    New-Server
    $kinds = @(
        @('show-options', '@nope'),
        @('show-options', '-w', '@nope'),
        @('show-options', '-g', 'nosuch'),
        @('show-options', '-gv', 'nosuch'),
        @('show-options', '-A', 'nosuch'),
        @('show-window-options', 'nosuch'),
        @('show-options', '-s', '@nope'),
        @('show-buffer', '-b', 'nosuch'),
        @('select-window', '-t', '99')
    )
    foreach ($k in $kinds) {
        P set -gu '@a' | Out-Null; P set -gu '@b' | Out-Null
        $out = P set -g '@a' 1 '\;' @k '\;' set -g '@b' 1
        $rc = $LASTEXITCODE
        $a = Opt '@a'; $b = Opt '@b'
        Check "$($k -join ' '): exit 1, @a set, @b not set" ($rc -eq 1 -and $a -eq '1' -and $b -eq '') "(exit $rc, @a='$a', @b='$b', out '$out')"
    }
    # A failure inside an if-shell branch ends the branch, not the outer list.
    P set -gu '@a' | Out-Null; P set -gu '@b' | Out-Null; P set -gu '@c' | Out-Null
    $out = P set -g '@a' 1 '\;' if-shell -F 1 'show-options @nope ; set -g @b 1' '\;' set -g '@c' 1
    $a = Opt '@a'; $b = Opt '@b'; $c = Opt '@c'
    Check "if-shell branch failure: @a and @c set, @b not" ($a -eq '1' -and $b -eq '' -and $c -eq '1') "(@a='$a', @b='$b', @c='$c')"
    # A list with no failure runs to the end.
    $out = P set -g '@x1' 1 '\;' set -g '@x2' 1 '\;' set -g '@x3' 1
    Check "a list with no failure runs all of it" ((Opt '@x3') -eq '1') ""
    # Config file: the failing line stops, the next line still loads
    # (cfg.c / cmd-parse.y: one group per line).
    $fa = Opt '@fa'; $fc = Opt '@fc'
    Check "config: the line after a failing line still loads (@fc)" ($fa -eq '1' -and $fc -eq '1') "(@fa='$fa', @fc='$fc')"
    Write-Host "    (config line's own rest: @fb='$(Opt '@fb')'; tmux leaves it unset, psmux's config loader is not part of #954)"
    # #951 follow up: a table that exists with a key it does not bind is
    # "unknown key" (cmd-list-keys.c:226-227), not a missing table.
    P bind -T mytbl x display hi | Out-Null
    $lk = (P list-keys -T mytbl y) -join ' '
    $lkRc = $LASTEXITCODE
    Check "list-keys -T mytbl y: exit 1, unknown key: y" ($lkRc -eq 1 -and $lk -match 'unknown key: y') "(exit $lkRc, '$lk')"
    $lk = (P list-keys -T nosuchtbl) -join ' '
    Check "list-keys -T nosuchtbl: table nosuchtbl doesn't exist" ($lk -match "table nosuchtbl doesn't exist") "('$lk')"
    Stop-Server

    # ------------------------------------------------- raw PERSISTENT line
    Write-Host "`n[2] PERSISTENT connection (the line the attached client sends)" -ForegroundColor Yellow
    New-Server
    $port = [int](Get-Content (Join-Path $TempHome "$($script:NS)__$S.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $TempHome "$($script:NS)__$S.key") -Raw).Trim()
    $tc = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $port)
    $st = $tc.GetStream(); $st.ReadTimeout = 300
    $w = [System.IO.StreamWriter]::new($st); $w.NewLine = "`n"; $w.AutoFlush = $true
    $r = [System.IO.StreamReader]::new($st)
    $w.WriteLine("AUTH $key"); [void]$r.ReadLine()
    $w.WriteLine("PERSISTENT"); $w.WriteLine("client-attach features=0x0")
    Start-Sleep -Milliseconds 300
    $mid = @('show-options @nope', 'show-options -g nosuch', 'show-buffer -b nosuch', 'select-window -t 99',
             'display-message -x hi', 'nosuchcommand a b', 'set-option nosuch x', 'split-window -t nosuch',
             'switch-client -t nosuch', 'kill-session -t nosuch', 'send-keys -t nosuch x', 'capture-pane -t nosuch')
    $i = 0
    foreach ($m in $mid) {
        $i++
        $w.WriteLine("set -g @pa$i 1 \; $m \; set -g @pb$i 1")
        $w.WriteLine("set -g @pc$i 1")
        Start-Sleep -Milliseconds 400
        $pa = Opt "@pa$i"; $pb = Opt "@pb$i"; $pc = Opt "@pc$i"
        Check "${m}: @pa set, @pb not set, the next line runs (@pc)" ($pa -eq '1' -and $pb -eq '' -and $pc -eq '1') "(@pa='$pa', @pb='$pb', @pc='$pc')"
    }
    $w.WriteLine("set -g @ia 1 \; if-shell -F 1 'show-options @nope ; set -g @ib 1' \; set -g @ic 1")
    Start-Sleep -Milliseconds 400
    $ia = Opt '@ia'; $ib = Opt '@ib'; $ic = Opt '@ic'
    Check "if-shell branch failure on the persistent connection: @ia and @ic set, @ib not" ($ia -eq '1' -and $ib -eq '' -and $ic -eq '1') "(@ia='$ia', @ib='$ib', @ic='$ic')"
    $w.WriteLine("set -g @sz0 1 \; source-file '$($sourced -replace '\\','/')' \; set -g @sz 1")
    Start-Sleep -Milliseconds 600
    $sa = Opt '@sa'; $sc = Opt '@sc'; $sz = Opt '@sz'
    Check "source-file: the line after a failing line still loads and the list goes on" ($sa -eq '1' -and $sc -eq '1' -and $sz -eq '1') "(@sa='$sa', @sc='$sc', @sz='$sz')"
    $tc.Close()
    Stop-Server

    # -------------------------------------------- real client, injector
    Write-Host "`n[3] attached client: prefix + : and key bindings (injector)" -ForegroundColor Yellow
    $toolDir = Join-Path $TempHome "tools"
    New-Item -ItemType Directory -Force $toolDir | Out-Null
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    $INJ = Join-Path $toolDir "injector.exe"
    $READ = Join-Path $toolDir "conread.exe"
    & $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
    if (-not ((Test-Path $INJ) -and (Test-Path $READ))) { Write-Fail "harness tools did not compile" }
    else {
        New-Server
        $script:client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $script:NS, "attach", "-t", $S -PassThru
        Start-Sleep -Seconds 4
        function Send-Keys($keys) { & $INJ $script:client.Id $keys | Out-Null; Start-Sleep -Milliseconds 900 }
        function Screen { ([string[]](& $READ $script:client.Id)) -join "`n" }

        Send-Keys "^b{SLEEP:500}:{SLEEP:600}set -g @ta 1 ; show-options @nope ; set -g @tb 1{ENTER}{SLEEP:1500}"
        $ta = Opt '@ta'; $tb = Opt '@tb'
        Check "prompt: @ta set, @tb not set" ($ta -eq '1' -and $tb -eq '') "(@ta='$ta', @tb='$tb')"
        Check "prompt: the error is on the status line" ((Screen).Contains('Invalid option: @nope')) ""
        Send-Keys "{ESC}{SLEEP:600}"

        Send-Keys "^b{SLEEP:500}:{SLEEP:600}set -g @ua 1 ; select-window -t 99 ; set -g @ub 1{ENTER}{SLEEP:1500}"
        $ua = Opt '@ua'; $ub = Opt '@ub'
        Check "prompt (bad target): @ua set, @ub not set" ($ua -eq '1' -and $ub -eq '') "(@ua='$ua', @ub='$ub')"
        Send-Keys "{ESC}{SLEEP:600}"

        Send-Keys "^b{SLEEP:500}:{SLEEP:600}set -g @va 1 ; set -g @vb 1 ; set -g @vc 1{ENTER}{SLEEP:1500}"
        Check "prompt: a list with no failure runs all of it" ((Opt '@vc') -eq '1' -and (Opt '@va') -eq '1') ""

        Send-Keys "{F9}{SLEEP:1200}"
        $ka = Opt '@ka'; $kb = Opt '@kb'
        Check "key binding: @ka set, @kb not set" ($ka -eq '1' -and $kb -eq '') "(@ka='$ka', @kb='$kb')"
        Send-Keys "{ESC}{SLEEP:600}"
        Send-Keys "{F10}{SLEEP:1200}"
        Check "key binding with no failure runs all of it" ((Opt '@ok1') -eq '1' -and (Opt '@ok2') -eq '1' -and (Opt '@ok3') -eq '1') ""
        Send-Keys "{F11}{SLEEP:2000}"
        Check "key binding: a foreground run-shell then a command still runs it (#740)" ((Opt '@rs') -eq '1') ""

        $mark = "CLS_TYPED_OK"
        Send-Keys "echo $mark{ENTER}{SLEEP:1500}"
        Check "typing still reaches the pane" ([bool]((P capture-pane -p -t $S) -match $mark)) ""
        Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue
        Start-Sleep -Milliseconds 500
        Stop-Server
    }
}
finally {
    if ($script:client -and -not $script:client.HasExited) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
