# Issue #842: the commands in a file run by source-file.
#
# The config parser that source-file uses applied options and bindings and
# dropped every other command except copy-mode, new-window and the select-pane
# input toggles. The ones it kept were sent back to the server and ran after
# the whole file, so a later line never saw what an earlier one made:
#
#   new-window -n "win-1"
#   split-window -h
#
# made win-1 and never split it. tmux 3.7c runs the file's commands one after
# another, right after the source-file item (cmd-source-file.c:119, cfg.c:155
# to :204), and each command without -t acts on what the one before it made
# current (cmd-new-window.c:147, cmd-split-window.c:217, cmd-find.c:975).
#
# Every expected value below is what tmux 3.7c printed for the same file on a
# fresh server (`tmux -L x -f /dev/null`). Checks run right after the CLI
# source-file returns, with no polling, because tmux's client returns only once
# the file's commands have run. The one exception is the shell output of a
# send-keys, which the shell writes when it gets to it.
#
# The suite runs under -L in a PSMUX_DATA_DIR of its own and kills only its own
# namespace. Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_sfc_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "sfc" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "sf"

Write-Host "`n=== Issue #842: source-file runs the file's commands in order ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
function Windows { ((P list-windows -t $S -F '#{window_index}=#{window_name}:#{window_panes}') -join ' ').Trim() }
function Opt($name) { ((P show-options -gv $name) -join '').Trim() }

# Run `source-file` from the CLI: its exit code, stdout and stderr.
function Source($path) {
    $out = Join-Path $TMP "out.txt"; $err = Join-Path $TMP "err.txt"
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "source-file", "`"$path`"" `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
    return @{
        Code = $p.ExitCode
        Out  = ((Get-Content $out -Raw -EA SilentlyContinue) + "").Trim()
        Err  = ((Get-Content $err -Raw -EA SilentlyContinue) + "").Trim()
    }
}

function Conf($name, [string[]]$lines) {
    $path = Join-Path $TMP $name
    $lines | Set-Content -Path $path -Encoding ASCII
    return $path
}

# A fresh session for each case, so window and pane numbers start over.
function Reset-Session {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 500
}

try {
    # ── A. the reported file ──
    Write-Head "A. new-window then split-window"
    $fA = Conf "a.conf" @('new-window -n "win-1"', 'split-window -h')
    Reset-Session
    $r = Source $fA
    Check ($r.Code -eq 0) "source-file exits 0" "exit $($r.Code), stderr [$($r.Err)]"
    $w = Windows
    Write-Info "windows: $w"
    Check ($w -match '(^| )1=win-1:2( |$)') "win-1 has two panes (tmux: 1=win-1:2)" "windows read [$w]"
    Check ($w -match '(^| )0=[^ ]*:1( |$)') "window 0 is not split" "windows read [$w]"
    $at = ((P display-message -t $S -p '#{window_name}.#{pane_index}') -join '').Trim()
    Check ($at -eq 'win-1.1') "the new pane is current (tmux: win-1.1)" "current is [$at]"

    # ── B. each line sees what the lines before it made ──
    # `.0` rather than tmux's bare `0`: psmux reads a bare number given to
    # select-pane's -t differently from tmux even on the CLI, which is not what
    # this suite is about. tmux 3.7c gives the same result for both.
    Write-Head "B. order across several commands"
    $fB = Conf "b.conf" @(
        'new-window -n ord',
        'split-window -h',
        'select-pane -t .0',
        'rename-window renamed',
        "send-keys 'echo MARK_B' Enter",
        "set -gF @seen '#{window_name}:#{window_panes}:#{pane_index}'"
    )
    Reset-Session
    $r = Source $fB
    Check ($r.Code -eq 0) "source-file exits 0" "exit $($r.Code), stderr [$($r.Err)]"
    $seen = Opt '@seen'
    Check ($seen -eq 'renamed:2:0') "@seen reads renamed:2:0 (tmux)" "@seen reads [$seen]"
    $mark = $false
    for ($i = 0; $i -lt 50 -and -not $mark; $i++) {
        $mark = [bool]((P capture-pane -p -t "${S}:renamed.0") -match 'MARK_B')
        if (-not $mark) { Start-Sleep -Milliseconds 100 }
    }
    Check $mark "send-keys reached pane 0 of the renamed window" "no MARK_B in renamed.0"

    # ── C. an option after a command sees the command's result ──
    Write-Head "C. a set after a split"
    $fC = Conf "c.conf" @('split-window', "set -gF @x '#{window_panes}'")
    Reset-Session
    $r = Source $fC
    $x = Opt '@x'
    Check ($x -eq '2') "@x reads 2 (tmux)" "@x reads [$x]"

    # ── D. a failing line ──
    Write-Head "D. a command that fails"
    $fD = Conf "d.conf" @('set -g @e1 a', 'split-window -t nosuch', 'set -g @e3 c')
    Reset-Session
    $r = Source $fD
    Write-Info "exit $($r.Code), stdout [$($r.Out)], stderr [$($r.Err)]"
    Check ($r.Code -eq 1) "source-file exits 1 (tmux)" "exit $($r.Code)"
    Check ($r.Err -match 'nosuch') "stderr names the target (tmux: can't find pane: nosuch)" "stderr [$($r.Err)]"
    Check ((Opt '@e1') -eq 'a') "line 1 ran" "@e1 reads [$(Opt '@e1')]"
    Check ((Opt '@e3') -eq 'c') "line 3 ran after the failure (tmux)" "@e3 reads [$(Opt '@e3')]"
    $log = Join-Path $TMP "config-warnings.log"
    $logText = if (Test-Path $log) { Get-Content $log -Raw } else { "" }
    Check ($logText.Contains("${fD}:2:")) "the warning names d.conf line 2" "config-warnings.log reads [$($logText.Trim())]"

    # ── E. a nested file ──
    Write-Head "E. a nested source-file"
    $fEc = Conf "e_child.conf" @('split-window -h', 'rename-window nested')
    $fE = Conf "e.conf" @('new-window -n nest', "source-file `"$fEc`"", 'set -g @after 1')
    Reset-Session
    $r = Source $fE
    $w = Windows
    Write-Info "windows: $w"
    Check ($w -match '(^| )1=nested:2( |$)') "the child split and renamed the parent's window (tmux: 1=nested:2)" "windows read [$w]"
    Check ((Opt '@after') -eq '1') "the parent's last line ran" "@after reads [$(Opt '@after')]"

    # ── F. the CLI returns once the commands have run ──
    Write-Head "F. the CLI's answer"
    $fF = Conf "f.conf" @('new-window -n p1', "display-message -p '#{window_name}'")
    Reset-Session
    $r = Source $fF
    Check ($r.Out -eq 'p1') "display-message -p printed p1 (tmux)" "stdout [$($r.Out)]"
    $ok = 0
    for ($k = 1; $k -le 5; $k++) {
        Reset-Session
        Source $fA | Out-Null
        if ((Windows) -match '(^| )1=win-1:2( |$)') { $ok++ }
    }
    Check ($ok -eq 5) "5 of 5 runs show the split as soon as the CLI returns" "$ok of 5"

    # ── G. the startup config is not a source ──
    Write-Head "G. commands in the startup config"
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    $fG = Conf "g.conf" @("set -g default-shell $POWERSHELL", 'new-window -n startw', 'split-window -h')
    $saved = $env:PSMUX_CONFIG_FILE
    $env:PSMUX_CONFIG_FILE = $fG
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $env:PSMUX_CONFIG_FILE = $saved
    Start-Sleep -Milliseconds 1500
    $w = Windows
    # tmux has no session while it runs the startup config and fails both
    # (0=...:1). A psmux session is a server that loads the config, so they
    # run, in order, on that session (tests/test_issue886_startup_config_run.ps1).
    Check ($w -match '^0=[^ ]*:1 1=startw:2$') "new-window then split-window, in order: 0=...:1 1=startw:2" "windows read [$w]"

    # ── H. a hook's source-file runs the file once ──
    Write-Head "H. source-file from a hook"
    $fH = Conf "h.conf" @('new-window -d -n hooked')
    Reset-Session
    P set-hook -g after-rename-window "source-file `"$fH`"" | Out-Null
    P rename-window -t "${S}:0" trigger | Out-Null
    $n = 0
    for ($i = 0; $i -lt 30; $i++) {
        $n = @((P list-windows -t $S -F '#{window_name}') | Where-Object { $_ -eq 'hooked' }).Count
        if ($n -ge 1) { break }
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 1500
    $n = @((P list-windows -t $S -F '#{window_name}') | Where-Object { $_ -eq 'hooked' }).Count
    Check ($n -eq 1) "exactly one hooked window" "$n hooked windows"

    # ── K. three levels of source-file ──
    # tmux 3.7c: 1=lvl3:2, and the lines after each nested source-file run.
    # Not checked: a format in the outermost file after the nested ones. tmux
    # keeps a current target per file's command list, so it still reads the
    # first window there (bash:1); psmux reads the window that is current.
    Write-Head "K. three levels of source-file"
    $fK3 = Conf "k3.conf" @('split-window -h', 'rename-window lvl3')
    $fK2 = Conf "k2.conf" @('new-window -n lvl2', "source-file `"$fK3`"", 'set -g @after2 1')
    $fK1 = Conf "k1.conf" @('set -g @before1 1', "source-file `"$fK2`"", 'set -g @after1 1')
    Reset-Session
    $r = Source $fK1
    $w = Windows
    Write-Info "windows: $w"
    Check ($r.Code -eq 0) "source-file exits 0" "exit $($r.Code), stderr [$($r.Err)]"
    Check ($w -match '(^| )1=lvl3:2( |$)') "the third file split and renamed the second's window (tmux: 1=lvl3:2)" "windows read [$w]"
    Check (((Opt '@before1') -eq '1') -and ((Opt '@after2') -eq '1') -and ((Opt '@after1') -eq '1')) `
        "every level's lines around its source-file ran" "@before1 [$(Opt '@before1')] @after2 [$(Opt '@after2')] @after1 [$(Opt '@after1')]"

    # ── I. a file that sources itself ──
    # psmux only, on purpose: tmux runs such a file down to its nesting limit
    # of 50 and then says "too many nested files". psmux refuses it before any
    # of its lines run. A command before the source-file line used to make the
    # source job run the file without end.
    Write-Head "I. a file that sources itself (psmux refuses it up front)"
    $fI = Join-Path $TMP "i.conf"
    @('set -g @self_ran 1', 'split-window -h', "source-file `"$fI`"") | Set-Content -Path $fI -Encoding ASCII
    Reset-Session
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Source $fI
    Write-Info "exit $($r.Code) after $($sw.ElapsedMilliseconds) ms, stderr [$($r.Err)]"
    Check ($r.Code -eq 1) "source-file exits 1" "exit $($r.Code)"
    Check ($r.Err -match 'is already being sourced') "stderr says the file is already being sourced" "stderr [$($r.Err)]"
    Check ($sw.ElapsedMilliseconds -lt 10000) "it answers at once" "it took $($sw.ElapsedMilliseconds) ms"
    $w = Windows
    # An option never set reads "invalid option: @self_ran" here, not empty.
    Check (($w -match '^0=[^ ]*:1$') -and ((Opt '@self_ran') -ne '1')) "none of its lines ran" "windows [$w], @self_ran [$(Opt '@self_ran')]"

    # ── J. a cycle through a second file ──
    Write-Head "J. a cycle through a second file (psmux only)"
    $fJa = Join-Path $TMP "ja.conf"; $fJb = Join-Path $TMP "jb.conf"
    @('new-window -n ja', "source-file `"$fJb`"") | Set-Content -Path $fJa -Encoding ASCII
    @('split-window -h', "source-file `"$fJa`"") | Set-Content -Path $fJb -Encoding ASCII
    Reset-Session
    $r = Source $fJa
    Write-Info "exit $($r.Code), stderr [$($r.Err)]"
    Check ($r.Code -eq 1) "source-file exits 1" "exit $($r.Code)"
    Check ($r.Err -match 'ja\.conf -> .*jb\.conf -> .*ja\.conf') "stderr shows the chain ja -> jb -> ja" "stderr [$($r.Err)]"
    $w = Windows
    Check ($w -match '^0=[^ ]*:1$') "none of the lines ran" "windows [$w]"

    # ── L. a source-file of itself that does not run at load time ──
    # tmux 3.7c loads all three at exit 0: the line runs only when the key is
    # pressed, or not at all.
    Write-Head "L. source-file of itself in a key binding, %if 0 and a false if-shell block"
    $fL1 = Join-Path $TMP "l1.conf"
    @('bind r {', "  source-file `"$fL1`"", '  display-message reloaded', '}', 'set -g @l1 1') | Set-Content -Path $fL1 -Encoding ASCII
    $fL2 = Join-Path $TMP "l2.conf"
    @('%if 0', "source-file `"$fL2`"", '%endif', 'set -g @l2 1') | Set-Content -Path $fL2 -Encoding ASCII
    $fL3 = Join-Path $TMP "l3.conf"
    @('if-shell false {', "  source-file `"$fL3`"", '}', 'set -g @l3 1') | Set-Content -Path $fL3 -Encoding ASCII
    Reset-Session
    foreach ($case in @(@{ F = $fL1; O = '@l1'; N = 'a bind block' }, @{ F = $fL2; O = '@l2'; N = '%if 0' }, @{ F = $fL3; O = '@l3'; N = 'if-shell false { }' })) {
        $r = Source $case.F
        Check (($r.Code -eq 0) -and ((Opt $case.O) -eq '1')) "$($case.N) loads at exit 0 (tmux)" "exit $($r.Code), stderr [$($r.Err)], $($case.O) [$(Opt $case.O)]"
    }
    $keys = ((P list-keys -T prefix r) -join ' ')
    Check ($keys -match 'source-file') "the reload key is bound" "list-keys reads [$keys]"

    # ── M. errors that are not a command's ──
    # tmux 3.7c: exit 1, the message on stderr, and the other lines still run.
    Write-Head "M. a missing nested file and an invalid option"
    $fM1 = Conf "m1.conf" @('set -g @m1a a', "source-file `"$(Join-Path $TMP 'nosuch.conf')`"", 'set -g @m1c c')
    Reset-Session
    $r = Source $fM1
    Write-Info "exit $($r.Code), stderr [$($r.Err)]"
    Check ($r.Code -eq 1) "a missing nested file exits 1 (tmux)" "exit $($r.Code)"
    Check ($r.Err -match 'nosuch\.conf') "stderr names the file" "stderr [$($r.Err)]"
    Check ((Opt '@m1c') -eq 'c') "the line after it ran (tmux)" "@m1c reads [$(Opt '@m1c')]"
    $fM2 = Conf "m2.conf" @('set -g @m2a a', 'set -g no-such-option 1', 'set -g @m2c c')
    $r = Source $fM2
    Write-Info "exit $($r.Code), stderr [$($r.Err)]"
    Check ($r.Code -eq 1) "an invalid option exits 1 (tmux)" "exit $($r.Code)"
    Check ($r.Err -match 'no-such-option') "stderr names the option" "stderr [$($r.Err)]"
    Check ((Opt '@m2c') -eq 'c') "the line after it ran (tmux)" "@m2c reads [$(Opt '@m2c')]"

    # ── N. -q ──
    Write-Head "N. source-file -q of a missing file"
    $fN = Conf "n.conf" @("source-file -q `"$(Join-Path $TMP 'nosuch.conf')`"", 'set -g @n1 1')
    $r = Source $fN
    Check (($r.Code -eq 0) -and ($r.Err -eq '') -and ((Opt '@n1') -eq '1')) "inside a file: exit 0, nothing on stderr (tmux)" "exit $($r.Code), stderr [$($r.Err)], @n1 [$(Opt '@n1')]"
    $out = Join-Path $TMP "nq_out.txt"; $err = Join-Path $TMP "nq_err.txt"
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "source-file", "-q", "`"$(Join-Path $TMP 'nosuch.conf')`"" `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
    $e = ((Get-Content $err -Raw -EA SilentlyContinue) + "").Trim()
    Check (($p.ExitCode -eq 0) -and ($e -eq '')) "from the CLI: exit 0, nothing on stderr (tmux)" "exit $($p.ExitCode), stderr [$e]"

    # ── O. -n ──
    # tmux 3.7c: parse only, nothing in the file runs (cfg.c:182). The CLI
    # dropped -n, so a syntax check ran the file's commands.
    Write-Head "O. source-file -n runs nothing"
    $fO = Conf "o.conf" @('new-window -n "o-win"', 'set -g @o1 1')
    Reset-Session
    $before = Windows
    foreach ($flags in @(@('-n'), @('-nq'), @('-q', '-n'))) {
        $out = Join-Path $TMP "no_out.txt"; $err = Join-Path $TMP "no_err.txt"
        $p = Start-Process -FilePath $PSMUX -ArgumentList (@("-L", $NS, "source-file") + $flags + "`"$fO`"") `
            -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
        Check ($p.ExitCode -eq 0) "source-file $($flags -join ' ') exits 0 (tmux)" "exit $($p.ExitCode)"
    }
    $after = Windows
    Check ($after -eq $before) "no window was made (tmux: $before)" "windows [$before] became [$after]"
    Check ((Opt '@o1') -ne '1') "no option was set (tmux)" "@o1 reads [$(Opt '@o1')]"
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
