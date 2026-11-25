# Issue #1080: `-T features` before the command, and every common CLI path
# around it.
#
# The flag became a value flag in the scans of the words before the command.
# This suite proves the value is never read as the command, that the other
# global flags (-L -f -S -u -2 -v -N, attached and `=` spellings, command
# lists) still combine with it, and that a `-T` AFTER the command still
# belongs to that command (bind-key -T, list-keys -T, unbind-key -T,
# select-pane -T, switch-client -T).
#
# Own -L namespace and PSMUX_DATA_DIR; honours PSMUX_TEST_BIN.
$ErrorActionPreference = 'Stop'
$Exe = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { Join-Path $PSScriptRoot '..\target\release\psmux.exe' }
$Exe = (Resolve-Path $Exe).Path
$env:PSMUX_RUN_NOPAUSE = '1'
$savedData = $env:PSMUX_DATA_DIR
$savedMode = $env:PSMUX_SERVER_MODE
$env:PSMUX_DATA_DIR = Join-Path $env:TEMP ('t1080cli-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $env:PSMUX_DATA_DIR | Out-Null
Remove-Item Env:PSMUX_SERVER_MODE -ErrorAction SilentlyContinue
foreach ($n in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION') { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
$ns = 't1080cli' + [guid]::NewGuid().ToString('N').Substring(0, 10)
$conf = Join-Path $env:PSMUX_DATA_DIR 'empty.conf'
Set-Content $conf ''
$script:pass = 0; $script:fail = 0
function Check([string]$label, [bool]$ok, [string]$detail = '') {
    if ($ok) { $script:pass++; Write-Host "PASS $label" }
    else { $script:fail++; Write-Host "FAIL $label $detail" -ForegroundColor Red }
}
function Run { $o = @(& $Exe @args 2>&1) -join "`n"; [pscustomobject]@{ Out = $o; Code = $LASTEXITCODE } }

try {
    $r = Run -L $ns -f $conf new-session -d -s a -x 80 -y 24
    Check 'new-session -d starts' ($r.Code -eq 0) $r.Out

    # -T before the command, in every place a global flag can stand.
    $cases = @(
        @('-T', '256,RGB', '-L', $ns, 'has-session', '-t', 'a'),
        @('-L', $ns, '-T', '256,RGB', 'has-session', '-t', 'a'),
        @('-L', $ns, '-TRGB', 'has-session', '-t', 'a'),
        @('-L', $ns, '-T=RGB', 'has-session', '-t', 'a'),
        @('-T', 'RGB', '-L', $ns, '-T', 'title', 'has-session', '-t', 'a'),
        @('-u', '-T', 'RGB', "-L$ns", '-2', 'has-session', '-t', 'a'),
        @('-L', $ns, '-f', $conf, '-T', 'hyperlinks', 'has-session', '-t', 'a'),
        @('-L', $ns, '-T', 'ls', 'has-session', '-t', 'a'),          # a value that is a command name
        @('-L', $ns, '-T', 'bogus', 'has-session', '-t', 'a'),       # an unknown feature is not an error (tmux)
        @('-L', $ns, '-T', '', 'has-session', '-t', 'a'),
        @('-L', $ns, '-N', '-T', 'RGB', 'has-session', '-t', 'a'),
        @('-L', $ns, '-v', '-T', 'RGB', 'has-session', '-t', 'a')
    )
    foreach ($c in $cases) {
        $r = Run @c
        Check ("exit 0: psmux " + ($c -join ' ')) ($r.Code -eq 0) "exit=$($r.Code) out=$($r.Out)"
    }
    $r = Run -L $ns -T '256,RGB' list-sessions -F '#{session_name}'
    Check '-T before list-sessions prints the session' ($r.Code -eq 0 -and $r.Out -eq 'a') "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns -T '256,RGB' -t a display-message -p '#{session_name}'
    Check '-T with a global -t' ($r.Code -eq 0 -and $r.Out -eq 'a') "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns -T RGB display-message -t a -p one '\;' display-message -t a -p two
    Check '-T before a command list' ($r.Code -eq 0 -and $r.Out -eq "one`ntwo") "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns -T
    Check '-T with no value is refused' ($r.Code -eq 1 -and $r.Out -match '-T expects an argument') "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns -T '256,RGB' attach -t '=nosuch'
    Check 'attach to a missing session names the session, not 256,RGB' ($r.Code -ne 0 -and $r.Out -notmatch 'unknown command') "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns -T RGB new-session -d -s b
    Check '-T before new-session -d' ($r.Code -eq 0) "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns has-session -t b
    Check 'that session exists' ($r.Code -eq 0) "exit=$($r.Code) out=$($r.Out)"

    # -T after the command belongs to the command.
    $r = Run -L $ns bind-key -T root F9 display-message hi1080
    Check 'bind-key -T root' ($r.Code -eq 0) "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns list-keys -T root
    Check 'list-keys -T root shows it' ($r.Code -eq 0 -and $r.Out -match 'F9.*hi1080') "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns -T RGB bind-key -Tcopy-mode-vi F8 send-keys -X cancel
    Check '-T global then bind-key -Tcopy-mode-vi' ($r.Code -eq 0) "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns list-keys -T copy-mode-vi
    Check 'list-keys -T copy-mode-vi shows F8' ($r.Code -eq 0 -and $r.Out -match 'F8') "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns bind-key -Troot F7 display-message hi7attached
    $k = Run -L $ns list-keys -T root
    Check 'bind-key -Troot (attached) binds in root' ($r.Code -eq 0 -and $k.Out -match 'F7.*hi7attached') "exit=$($r.Code) keys=$($k.Out)"
    $r = Run -L $ns bind-key -rT root F6 display-message hi6
    $k = Run -L $ns list-keys -T root
    Check 'bind-key -rT root (cluster) binds repeatable in root' ($k.Out -match 'bind-key -r -T root F6') "exit=$($r.Code) keys=$($k.Out)"
    $k = Run -L $ns list-keys -Troot
    Check 'list-keys -Troot (attached)' ($k.Code -eq 0 -and $k.Out -match 'hi7attached') "exit=$($k.Code) keys=$($k.Out)"
    $r = Run -L $ns unbind-key -Troot F7
    $k = Run -L $ns list-keys -T root
    Check 'unbind-key -Troot (attached)' ($r.Code -eq 0 -and $k.Out -notmatch 'hi7attached') "exit=$($r.Code) keys=$($k.Out)"
    $cf = Join-Path $env:PSMUX_DATA_DIR 'bind.conf'
    Set-Content $cf "bind -Troot F5 display-message hi5conf`nbind -rTroot F4 display-message hi4conf"
    $r = Run -L $ns source-file $cf
    $k = Run -L $ns list-keys -T root
    Check 'config bind -Troot binds in root, not -r prefix' ($k.Out -match 'bind-key -T root F5 display-message hi5conf') "keys=$($k.Out)"
    Check 'config bind -rTroot binds repeatable in root' ($k.Out -match 'bind-key -r -T root F4 display-message hi4conf') "keys=$($k.Out)"
    $r = Run -L $ns unbind-key -T root F9
    Check 'unbind-key -T root' ($r.Code -eq 0) "exit=$($r.Code) out=$($r.Out)"
    $r = Run -L $ns list-keys -T root
    Check 'F9 gone from root' ($r.Out -notmatch 'hi1080') "out=$($r.Out)"
    $r = Run -L $ns select-pane -t a -T mytitle1080
    $t = Run -L $ns display-message -t a -p '#{pane_title}'
    Check 'select-pane -T sets the pane title' ($r.Code -eq 0 -and $t.Out -eq 'mytitle1080') "exit=$($r.Code) title=$($t.Out)"
    $r = Run -L $ns -T RGB select-pane -t a -Ttitle2
    $t = Run -L $ns display-message -t a -p '#{pane_title}'
    Check 'global -T then select-pane -Ttitle2' ($r.Code -eq 0 -and $t.Out -eq 'title2') "exit=$($r.Code) title=$($t.Out)"
    $r = Run -L $ns split-window -t a -d
    $p = Run -L $ns display-message -t a -p '#{window_panes}'
    Check 'split-window still works' ($r.Code -eq 0 -and $p.Out -eq '2') "exit=$($r.Code) panes=$($p.Out)"
    $r = Run -L $ns -T RGB new-window -t a -d
    $w = Run -L $ns display-message -t a -p '#{session_windows}'
    Check '-T before new-window' ($r.Code -eq 0 -and $w.Out -eq '2') "exit=$($r.Code) windows=$($w.Out)"
} finally {
    & $Exe -L $ns kill-server 2>$null | Out-Null
    $env:PSMUX_DATA_DIR = $savedData
    if ($savedMode) { $env:PSMUX_SERVER_MODE = $savedMode }
}
Write-Host "passed=$script:pass failed=$script:fail"
if ($script:fail -gt 0) { exit 1 }
exit 0
