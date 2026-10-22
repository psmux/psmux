# Issue #886: the startup config runs its commands, and a run-shell in it
# holds the lines after it.
#
# tmux runs the whole config through its command queue before the first
# client's command (cfg.c:71 to :92, tag 3.7c), so every command in it runs,
# and a run-shell without -b holds the lines after it (cmd-run-shell.c:123,
# :186 to :188). A new-session there has no client, so it is detached
# (cmd-new-session.c:164 to :167).
#
# psmux dropped every command in the startup config that is not a config
# directive, without a word: split-window, new-window, set-buffer and
# new-session did nothing, and run-shell never waited. It now runs the config
# the way a runtime source-file runs a file (issue #842): options before the
# first command apply at once, and the commands and every line after them run
# in order on a source job once the session's window exists.
#
# Where psmux differs from tmux on purpose, the check says so:
#   - each psmux session is a server that loads the config, so a command that
#     needs a session acts on the session that loads it (tmux has none yet
#     and fails it);
#   - a session the config's new-session makes runs, of the config's
#     commands, only run-shell and the buffer commands: tmux runs its config
#     once per server, and that session would share what they made there;
#   - a warm standby starts no run-shell of the config: its claim does, and
#     it used to start them as well, so they ran twice;
#   - new-session -d returns before the commands have run.
#
# The suite runs under -L in a PSMUX_DATA_DIR of its own and kills only its own
# namespace. Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL',
               'PSMUX_CONFIG_SESSION', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_scr_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "scr" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "s"
$CONF = Join-Path $TMP "startup.conf"
$env:PSMUX_CONFIG_FILE = $CONF
# The binary a shell command calls back into, quoted for PowerShell.
$CALL = "& '$PSMUX' -L $NS"

Write-Host "`n=== The startup config runs its commands ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
function Opt($name, $t = $S) { ((P show-options -t $t -gv $name) -join '').Trim() }
function Sessions { @(P list-sessions -F '#{session_name}' | Where-Object { $_ -and $_ -notmatch 'no server' } | Sort-Object) }

# Wait until $cond is true, up to $ms milliseconds.
function Wait-Until([scriptblock]$cond, [int]$ms = 8000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        if (& $cond) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return (& $cond)
}

# Kill the namespace, write the config and start session s detached. Returns
# how long new-session -d took, in milliseconds.
function Start-WithConfig([string[]]$lines) {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 600
    Get-ChildItem $TMP -Filter "*.port" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
    (@("set -g default-shell $POWERSHELL") + $lines) -join "`n" | Set-Content -Path $CONF -Encoding ASCII
    $sw = [Diagnostics.Stopwatch]::StartNew()
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $ms = $sw.ElapsedMilliseconds
    Wait-Until { Test-Path (Join-Path $TMP "${NS}__${S}.port") } 6000 | Out-Null
    return $ms
}

try {
    Write-Head "A. commands that need a session act on the session that loads the config"
    Start-WithConfig @('set -g @before on', 'rename-window cfgname', 'split-window -h', 'new-window -n cfgwin', 'set -g @after on') | Out-Null
    $done = Wait-Until { (Opt '@after') -eq 'on' }
    Check $done "the line after the commands ran" "@after reads [$(Opt '@after')]"
    Check ((Opt '@before') -eq 'on') "the option before them applied" "@before reads [$(Opt '@before')]"
    $wins = (P list-windows -t $S -F '#{window_name}:#{window_panes}') -join ' '
    Check ($wins -eq 'cfgname:2 cfgwin:1') "windows are cfgname:2 cfgwin:1 (tmux has no session there and fails them)" "windows are [$wins]"

    Write-Head "A2. select-pane -d and copy-mode run on the job like any other command"
    Start-WithConfig @('split-window -h', 'select-pane -d -t :.0', 'copy-mode -t :.1', 'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
    $panes = (P list-panes -t $S -F '#{pane_index}:#{pane_input_off}:#{pane_in_mode}') -join ' '
    Check ($panes -eq '0:1:0 1:0:1') "pane 0 input off, pane 1 in copy mode (tmux has no session there and fails them)" "panes read [$panes]"

    Write-Head "B. a command that needs no session"
    Start-WithConfig @('set-buffer -b cfgbuf from-config', 'setenv -g CFGENV2 yes', 'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
    $buf = ((P show-buffer -b cfgbuf) -join '').Trim()
    Check ($buf -eq 'from-config') "buffer cfgbuf holds from-config (tmux)" "buffer cfgbuf reads [$buf]"
    $envv = ((P show-environment -g CFGENV2) -join '').Trim()
    Check ($envv -eq 'CFGENV2=yes') "CFGENV2=yes (tmux)" "show-environment reads [$envv]"

    Write-Head "C. the line after a run-shell waits for it"
    $ms = Start-WithConfig @('run-shell "Start-Sleep -Milliseconds 2000"', 'set -g @after on')
    Start-Sleep -Milliseconds 300
    $early = Opt '@after'
    Check ($early -ne 'on') "@after is not set while the shell runs" "@after already reads [$early], new-session took $ms ms"
    Check (Wait-Until { (Opt '@after') -eq 'on' }) "@after is set once the shell is done (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "D. a shell command that calls back runs before the next line"
    Start-WithConfig @("run-shell `"Start-Sleep -Milliseconds 800; $CALL set -g '@x' a`"", 'set -ga @x b', 'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
    Check ((Opt '@x') -eq 'ab') "@x is ab (tmux)" "@x reads [$(Opt '@x')]"

    Write-Head "E. -d with no command only waits"
    Start-WithConfig @('run-shell -d 2', 'set -g @after on') | Out-Null
    Start-Sleep -Milliseconds 300
    Check ((Opt '@after') -ne 'on') "@after is not set during the delay" "@after already reads [$(Opt '@after')]"
    Check (Wait-Until { (Opt '@after') -eq 'on' }) "@after is set after the delay (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "F. -b does not wait"
    Start-WithConfig @('run-shell -b "Start-Sleep -Milliseconds 3000"', 'set -g @after on') | Out-Null
    Check (Wait-Until { (Opt '@after') -eq 'on' } 1500) "@after is set at once (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "G. new-session -d -s cfg makes cfg, which runs only run-shell and the buffer commands"
    Start-WithConfig @('new-session -d -s cfg', 'new-window -n fromcfg', 'set-buffer -b cfgbuf shared',
        "run-shell `"$CALL set -ga '@runs' x`"", 'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
    Check ((Opt '@runs') -eq 'x') "s ran the run-shell once" "@runs in s reads [$(Opt '@runs')]"
    Check (Wait-Until { (Sessions) -contains 'cfg' }) "session cfg exists (tmux)" "sessions are [$((Sessions) -join ' ')]"
    $sw = (P list-windows -t $S -F '#{window_name}') -join ' '
    Check ($sw -match 'fromcfg') "new-window after it ran in s" "s has [$sw]"
    Start-Sleep -Milliseconds 1500
    $cw = @(P list-windows -t cfg -F '#{window_name}')
    Check (($cw.Count -eq 1) -and (($cw -join ' ') -notmatch 'fromcfg')) "cfg has one window: it did not run new-window" "cfg has [$($cw -join ' ')]"
    Wait-Until { (Opt '@after' 'cfg') -eq 'on' } | Out-Null
    Check ((Opt '@runs' 'cfg') -eq 'x') "cfg ran the run-shell once (tmux: the one server's run-shell set it)" "@runs in cfg reads [$(Opt '@runs' 'cfg')]"
    $cbuf = ((P show-buffer -t cfg -b cfgbuf) -join '').Trim()
    Check ($cbuf -eq 'shared') "cfg has buffer cfgbuf (tmux: one buffer list per server)" "buffer cfgbuf in cfg reads [$cbuf]"
    Check (@(Sessions).Count -eq 2) "two sessions, s and cfg" "sessions are [$((Sessions) -join ' ')]"

    Write-Head "H. new-session -d with no name makes one session, not one per server"
    Start-WithConfig @('new-session -d', 'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
    Wait-Until { @(Sessions).Count -ge 2 } | Out-Null
    Start-Sleep -Milliseconds 3000
    Check (@(Sessions).Count -eq 2) "two sessions after 3 s" "sessions are [$((Sessions) -join ' ')]"

    Write-Head "I. attach-session in the config does not stop the lines after it"
    Start-WithConfig @('attach-session', 'set -g @after on') | Out-Null
    Check (Wait-Until { (Opt '@after') -eq 'on' }) "@after is set (tmux)" "@after reads [$(Opt '@after')]"

    Write-Head "J. a command that fails is a warning at its own line"
    Start-WithConfig @('select-window -t :9', 'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
    $log = Join-Path $TMP "config-warnings.log"
    $okLog = Wait-Until { (Test-Path $log) -and ((Get-Content $log -Raw) -match 'startup\.conf:2: .*9') } 4000
    $logText = if (Test-Path $log) { (Get-Content $log -Raw).Trim() } else { '(none)' }
    Check $okLog "config-warnings.log names startup.conf:2" "config-warnings.log reads [$logText]"

    Write-Head "J2. a plugin script's source-file at startup runs its commands too"
    # A `set -g @plugin` .ps1 that calls back into psmux runs while the server
    # starts, before it has a window. tmux runs the commands of a file a
    # plugin sources (with run-shell -b, once the session exists).
    $PHOME = Join-Path $TMP "plughome"
    $PDIR = Join-Path $PHOME ".psmux\plugins\plugsrc"
    New-Item -ItemType Directory -Force $PDIR | Out-Null
    $PCONF = Join-Path $TMP "plug.conf"
    @('set-buffer -b plugbuf from-plugin', 'new-window -n plugwin', 'set -g @plugdone on') -join "`n" |
        Set-Content -Path $PCONF -Encoding ASCII
    "& '$PSMUX' source-file '$PCONF'" | Set-Content -Path (Join-Path $PDIR "plugsrc.ps1") -Encoding ASCII
    $savedHome = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME }
    $env:USERPROFILE = $PHOME; $env:HOME = $PHOME
    try {
        Start-WithConfig @("set -g @plugin 'x/plugsrc'", 'set -g @after on') | Out-Null
        Check (Wait-Until { (Opt '@plugdone') -eq 'on' }) "the plugin's file ran (@plugdone on)" "@plugdone reads [$(Opt '@plugdone')]"
        Start-Sleep -Milliseconds 1000
        $buf = ((P show-buffer -b plugbuf) -join '').Trim()
        Check ($buf -eq 'from-plugin') "buffer plugbuf holds from-plugin (tmux)" "buffer plugbuf reads [$buf]"
        $wins = (P list-windows -t $S -F '#{window_name}') -join ' '
        Check ($wins -match 'plugwin') "new-window from the plugin's file made plugwin (tmux, with run-shell -b)" "windows are [$wins]"
    } finally {
        foreach ($k in $savedHome.Keys) {
            if ($null -ne $savedHome[$k]) { Set-Item "Env:$k" $savedHome[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    Write-Head "J3. a plugin declared after the config's first command runs its script"
    # Its `set -g @plugin` line is replayed by the job, after the startup
    # drain that runs queued plugin scripts; the script stayed queued and
    # never ran. A runtime source-file's @plugin line never ran its script
    # either.
    $RDIR = Join-Path $PHOME ".psmux\plugins\plugrun"
    New-Item -ItemType Directory -Force $RDIR | Out-Null
    @('$v = ''yes''', "& '$PSMUX' set -g '@plugran' `$v") -join "`n" |
        Set-Content -Path (Join-Path $RDIR "plugrun.ps1") -Encoding ASCII
    $RCONF = Join-Path $TMP "plugrun.conf"
    "set -g @plugin 'x/plugrun'" | Set-Content -Path $RCONF -Encoding ASCII
    $savedHome = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME }
    $env:USERPROFILE = $PHOME; $env:HOME = $PHOME
    try {
        Start-WithConfig @('run-shell "exit 0"', "set -g @plugin 'x/plugrun'", 'set -g @after on') | Out-Null
        Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
        Check (Wait-Until { (Opt '@plugran') -eq 'yes' }) "the plugin's script ran in s" "@plugran reads [$(Opt '@plugran')]"
        P set -g '@plugran' no | Out-Null
        P source-file $RCONF | Out-Null
        Check (Wait-Until { (Opt '@plugran') -eq 'yes' }) "a runtime source-file's @plugin runs the script too" "@plugran reads [$(Opt '@plugran')]"
    } finally {
        foreach ($k in $savedHome.Keys) {
            if ($null -ne $savedHome[$k]) { Set-Item "Env:$k" $savedHome[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    Write-Head "J4. a plugin.conf that declares its own plugin is read once"
    # psmux reads a declared plugin's plugin.conf itself. One that declares
    # its own plugin again recursed until the server died, so the session
    # was never made.
    $SDIR = Join-Path $PHOME ".psmux\plugins\plugself"
    New-Item -ItemType Directory -Force $SDIR | Out-Null
    @('set -ga @selfreads x', "set -g @plugin 'x/plugself'") -join "`n" |
        Set-Content -Path (Join-Path $SDIR "plugin.conf") -Encoding ASCII
    $savedHome = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME }
    $env:USERPROFILE = $PHOME; $env:HOME = $PHOME
    try {
        Start-WithConfig @("set -g @plugin 'x/plugself'", 'set -g @after on') | Out-Null
        Check (Wait-Until { (Opt '@after') -eq 'on' }) "the session starts" "@after reads [$(Opt '@after')]"
        Check ((Opt '@selfreads') -eq 'x') "the plugin.conf was read once" "@selfreads reads [$(Opt '@selfreads')]"
    } finally {
        foreach ($k in $savedHome.Keys) {
            if ($null -ne $savedHome[$k]) { Set-Item "Env:$k" $savedHome[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    Write-Head "J5. with ppm declared, only ppm runs a plugin's script"
    # ppm runs the entry script of each declared plugin that has no
    # plugin.conf, from the config's `run` line, as tpm runs each plugin in
    # tmux (tpm scripts/source_plugins.sh). psmux ran the script as well, so
    # it ran twice. The `run` line here stands in for ppm's.
    $CDIR = Join-Path $PHOME ".psmux\plugins\plugcount"
    New-Item -ItemType Directory -Force $CDIR | Out-Null
    @('$x = ''x''', "& '$PSMUX' set -ga '@plugruns' `$x") -join "`n" |
        Set-Content -Path (Join-Path $CDIR "plugcount.ps1") -Encoding ASCII
    $savedHome = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME }
    $env:USERPROFILE = $PHOME; $env:HOME = $PHOME
    try {
        foreach ($case in @(
                @{ Name = 'before'; Lines = @("set -g @plugin 'psmux-plugins/ppm'", "set -g @plugin 'x/plugcount'") },
                @{ Name = 'after'; Lines = @("set -g @plugin 'x/plugcount'", "set -g @plugin 'psmux-plugins/ppm'") })) {
            Start-WithConfig ($case.Lines + @("run '~/.psmux/plugins/plugcount/plugcount.ps1'", 'set -g @after on')) | Out-Null
            Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
            Start-Sleep -Milliseconds 1500
            Check ((Opt '@plugruns') -eq 'x') "ppm declared $($case.Name) the plugin: the script ran once" "@plugruns reads [$(Opt '@plugruns')]"
        }
    } finally {
        foreach ($k in $savedHome.Keys) {
            if ($null -ne $savedHome[$k]) { Set-Item "Env:$k" $savedHome[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    Write-Head "J6. a plugin script psmux cannot apply as config lines runs whole, once"
    # psmux applies an entry script that only sets options and binds keys as
    # config lines. One with a variable had its lines before the variable
    # applied, then all of them again when it ran; one with another command
    # lost that command.
    $ADIR = Join-Path $PHOME ".psmux\plugins\plugpart"
    New-Item -ItemType Directory -Force $ADIR | Out-Null
    @("`$PSMUX = '$PSMUX'", '& $PSMUX set -ga ''@part'' a', '$v = ''b''', '& $PSMUX set -ga ''@part'' $v') -join "`n" |
        Set-Content -Path (Join-Path $ADIR "plugpart.ps1") -Encoding ASCII
    $ODIR = Join-Path $PHOME ".psmux\plugins\plugother"
    New-Item -ItemType Directory -Force $ODIR | Out-Null
    @("`$PSMUX = '$PSMUX'", '& $PSMUX set -g ''@other'' on', '& $PSMUX set-buffer -b otherbuf yes') -join "`n" |
        Set-Content -Path (Join-Path $ODIR "plugother.ps1") -Encoding ASCII
    # Sourced once the session runs: a script that runs before the session's
    # window exists has only its option, key and source-file calls served.
    $OCONF = Join-Path $TMP "plugother.conf"
    "set -g @plugin 'x/plugother'" | Set-Content -Path $OCONF -Encoding ASCII
    $savedHome = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME }
    $env:USERPROFILE = $PHOME; $env:HOME = $PHOME
    try {
        Start-WithConfig @("set -g @plugin 'x/plugpart'", 'set -g @after on') | Out-Null
        Wait-Until { (Opt '@after') -eq 'on' } | Out-Null
        Wait-Until { (Opt '@part').Length -ge 2 } | Out-Null
        Start-Sleep -Milliseconds 1000
        Check ((Opt '@part') -eq 'ab') "@part is ab: no line was applied twice" "@part reads [$(Opt '@part')]"
        P source-file $OCONF | Out-Null
        $okBuf = Wait-Until { ((P show-buffer -b otherbuf) -join '').Trim() -eq 'yes' } 5000
        Check $okBuf "buffer otherbuf holds yes: the set-buffer line ran" "buffer otherbuf reads [$(((P show-buffer -b otherbuf) -join '').Trim())]"
        Check ((Opt '@other') -eq 'on') "@other is on" "@other reads [$(Opt '@other')]"
    } finally {
        foreach ($k in $savedHome.Keys) {
            if ($null -ne $savedHome[$k]) { Set-Item "Env:$k" $savedHome[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    Write-Head "K. an attached client stays on its session when the config makes another"
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 600
    (@("set -g default-shell $POWERSHELL", 'new-session -s other', 'set -g @after on')) -join "`n" | Set-Content -Path $CONF -Encoding ASCII
    $proc = Start-Process -FilePath $PSMUX -ArgumentList '-L', $NS, 'new-session', '-s', $S -PassThru
    Wait-Until { (Opt '@after') -eq 'on' } 10000 | Out-Null
    Check (Wait-Until { (Sessions) -contains 'other' }) "session other exists (tmux)" "sessions are [$((Sessions) -join ' ')]"
    Start-Sleep -Milliseconds 1500
    $clients = (P list-clients -t $S -F '#{client_session}') -join ' '
    Check ($clients -match "(^|\s)$S(\s|$)") "the client is still on s (tmux: no client at config time, so no switch)" "list-clients -t s reads [$clients]"
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }

    Write-Head "L. a claimed warm server runs the config's commands once, and its standby none"
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 600
    $HOME_DIR = Join-Path $TMP "home"
    New-Item -ItemType Directory -Force $HOME_DIR | Out-Null
    $RUNLOG = Join-Path $TMP "runlog.txt"
    (@("set -g default-shell $POWERSHELL", "run-shell -b `"Add-Content -Path '$RUNLOG' -Value x`"",
        'split-window -h', 'set -g @after on')) -join "`n" |
        Set-Content -Path (Join-Path $HOME_DIR ".psmux.conf") -Encoding ASCII
    $runs = { if (Test-Path $RUNLOG) { @(Get-Content $RUNLOG).Count } else { 0 } }
    $saved = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE; PSMUX_NO_WARM = $env:PSMUX_NO_WARM }
    $env:USERPROFILE = $HOME_DIR; $env:HOME = $HOME_DIR
    Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
    Remove-Item Env:PSMUX_NO_WARM -EA SilentlyContinue
    try {
        P new-session -d -s first -x 120 -y 30 | Out-Null
        $warmPort = Join-Path $TMP "${NS}____warm__.port"
        $standby = Wait-Until { Test-Path $warmPort } 15000
        Check $standby "a warm standby is up" "no standby port file at $warmPort"
        Start-Sleep -Milliseconds 2500
        $n = & $runs
        Check ($n -eq 1) "the config's run-shell ran once: the standby started none" "it ran $n times"
        P new-session -d -s second | Out-Null
        Check (Wait-Until { (Opt '@after' 'second') -eq 'on' }) "the claimed session ran the line after split-window" "@after reads [$(Opt '@after' 'second')]"
        Start-Sleep -Milliseconds 1000
        $panes = ((P list-windows -t second -F '#{window_panes}') -join ' ').Trim()
        Check ($panes -eq '2') "the claimed session has 2 panes: split-window ran once" "second has [$panes] panes"
        $panes1 = ((P list-windows -t first -F '#{window_panes}') -join ' ').Trim()
        Check ($panes1 -eq '2') "the cold session has 2 panes too" "first has [$panes1] panes"
        Start-Sleep -Milliseconds 2500
        $n = & $runs
        Check ($n -eq 2) "the claim ran it once more, and the next standby none" "it ran $n times"
    } finally {
        foreach ($k in $saved.Keys) {
            if ($null -ne $saved[$k]) { Set-Item "Env:$k" $saved[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    Write-Head "M. each session runs its plugin scripts once, and their psmux calls reach it"
    # A plugin script got no TMUX, so its psmux calls went to the most recent
    # session. A warm standby ran the scripts at its boot, for the session
    # before it, and its claim ran none.
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 600
    $WLOG = Join-Path $TMP "wherelog.txt"
    $WDIR = Join-Path $HOME_DIR ".psmux\plugins\plugwhere"
    New-Item -ItemType Directory -Force $WDIR | Out-Null
    @("Add-Content -Path '$WLOG' -Value x", "& '$PSMUX' set -ga '@where' x") -join "`n" |
        Set-Content -Path (Join-Path $WDIR "plugwhere.ps1") -Encoding ASCII
    (@("set -g default-shell $POWERSHELL", "set -g @plugin 'x/plugwhere'", 'set -g @after on')) -join "`n" |
        Set-Content -Path (Join-Path $HOME_DIR ".psmux.conf") -Encoding ASCII
    $runs = { if (Test-Path $WLOG) { @(Get-Content $WLOG).Count } else { 0 } }
    $saved = @{ USERPROFILE = $env:USERPROFILE; HOME = $env:HOME; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE; PSMUX_NO_WARM = $env:PSMUX_NO_WARM }
    $env:USERPROFILE = $HOME_DIR; $env:HOME = $HOME_DIR
    Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
    Remove-Item Env:PSMUX_NO_WARM -EA SilentlyContinue
    try {
        P new-session -d -s first -x 120 -y 30 | Out-Null
        $standby = Wait-Until { Test-Path (Join-Path $TMP "${NS}____warm__.port") } 15000
        Check $standby "a warm standby is up" "no standby port file"
        Start-Sleep -Milliseconds 2500
        $n = & $runs
        Check ($n -eq 1) "the script ran once: the standby ran none" "it ran $n times"
        Check ((Opt '@where' 'first') -eq 'x') "its call reached first" "@where in first reads [$(Opt '@where' 'first')]"
        P new-session -d -s second | Out-Null
        Wait-Until { (Opt '@where' 'second') -eq 'x' } | Out-Null
        Start-Sleep -Milliseconds 2500
        $n = & $runs
        Check ($n -eq 2) "the claim ran it once more, and the next standby none" "it ran $n times"
        Check ((Opt '@where' 'second') -eq 'x') "its call reached second" "@where in second reads [$(Opt '@where' 'second')]"
        Check ((Opt '@where' 'first') -eq 'x') "and none reached first again" "@where in first reads [$(Opt '@where' 'first')]"
    } finally {
        foreach ($k in $saved.Keys) {
            if ($null -ne $saved[$k]) { Set-Item "Env:$k" $saved[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    # A tmux plugin's entry script, as tpm runs it: bash, with a macOS-only
    # line that must not apply here, and calls back into the server.
    $TDIR = Join-Path $TMP "tplug"
    New-Item -ItemType Directory -Force $TDIR | Out-Null
    $TSCRIPT = (Join-Path $TDIR "tplug.tmux").Replace('\', '/')
    @(
        '#!/usr/bin/env bash'
        'PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"'
        'tmux source "${PLUGIN_DIR}/extra.conf"'
        'main() {'
        '  sleep 1'
        '  tmux set -g @inner "$(tmux show -gv @before)-seen"'
        '}'
        'if [ "$(uname)" = "Darwin" ]; then'
        '  tmux set -g default-command "reattach-to-user-namespace -l $SHELL"'
        'fi'
        'main'
    ) -join "`n" | Set-Content -Path $TSCRIPT -Encoding ASCII -NoNewline
    @('set -g @extra on', 'source -F "#{d:current_file}/more.conf"') -join "`n" |
        Set-Content -Path (Join-Path $TDIR "extra.conf") -Encoding ASCII
    'set -g @more on' | Set-Content -Path (Join-Path $TDIR "more.conf") -Encoding ASCII
    $TLINES = @('set -g @before on', "run '$TSCRIPT'", 'set -gF @after "#{@inner}"')

    Write-Head "N. a .tmux entry script runs with Git Bash, and the lines after it wait"
    $GITBASH = $null
    $git = Get-Command git -EA SilentlyContinue
    $roots = @()
    if ($git) { $d = Split-Path $git.Source; $roots += @($d, (Split-Path $d), (Split-Path (Split-Path $d))) }
    $roots += @((Join-Path $env:ProgramFiles 'Git'), (Join-Path $env:LOCALAPPDATA 'Programs\Git'))
    foreach ($root in $roots) {
        if ($root -and (Test-Path "$root\bin\bash.exe") -and (Test-Path "$root\usr\bin\bash.exe")) { $GITBASH = "$root\bin\bash.exe"; break }
    }
    if (-not $GITBASH) {
        Write-Host "  [SKIP] Git for Windows is not installed" -ForegroundColor DarkYellow
    } else {
        Start-WithConfig $TLINES | Out-Null
        Check (Wait-Until { (Opt '@after') -notmatch 'invalid option' } 15000) "the line after it ran" "@after reads [$(Opt '@after')]"
        Check ((Opt '@after') -eq 'on-seen') "it ran after the script (tmux): @after is on-seen" "@after reads [$(Opt '@after')]"
        Check ((Opt '@extra') -eq 'on' -and (Opt '@more') -eq 'on') "the script's source and the #{d:current_file} in it applied" "@extra [$(Opt '@extra')] @more [$(Opt '@more')]"
        Check ((Opt 'default-command') -notmatch 'reattach') "the macOS-only line did not apply" "default-command reads [$(Opt 'default-command')]"
    }

    Write-Head "O. without Git Bash, only the script's plain tmux lines apply, with a warning"
    $saved = @{ PATH = $env:PATH; ProgramFiles = $env:ProgramFiles; ProgramW6432 = $env:ProgramW6432; LOCALAPPDATA = $env:LOCALAPPDATA; USERPROFILE = $env:USERPROFILE }
    $env:PATH = ($env:PATH -split ';' | Where-Object { $_ -and $_ -notmatch '\\Git\\' }) -join ';'
    foreach ($k in 'ProgramFiles', 'ProgramW6432', 'LOCALAPPDATA', 'USERPROFILE') { Set-Item "Env:$k" (Join-Path $TMP "nogit") }
    try {
        Start-WithConfig $TLINES | Out-Null
        Check (Wait-Until { (Opt '@before') -eq 'on' } 8000) "the session started" "@before reads [$(Opt '@before')]"
        Check ((Opt '@extra') -eq 'on' -and (Opt '@more') -eq 'on') "the plain source line and the #{d:current_file} in it applied" "@extra [$(Opt '@extra')] @more [$(Opt '@more')]"
        Check ((Opt '@inner') -match 'invalid option') "the function body did not apply" "@inner reads [$(Opt '@inner')]"
        Check ((Opt 'default-command') -notmatch 'reattach') "the macOS-only line did not apply: panes start" "default-command reads [$(Opt 'default-command')]"
        $log = (Get-Content (Join-Path $TMP 'config-warnings.log') -EA SilentlyContinue) -join ' '
        Check ($log -match 'needs Git Bash') "a warning says the script needs Git Bash" "config-warnings.log reads [$log]"
    } finally {
        foreach ($k in $saved.Keys) {
            if ($null -ne $saved[$k]) { Set-Item "Env:$k" $saved[$k] } else { Remove-Item "Env:$k" -EA SilentlyContinue }
        }
    }

    # In tmux each command parsed from a file carries the file as
    # #{current_file} (cfg.c:139 and :191), so a run-shell after the file's
    # first command still names it. The job ran its commands with none.
    Write-Head "Q. a command after the first one still has #{current_file}"
    Start-WithConfig @('split-window',
        "run-shell `"$CALL set -g '@cf' '#{current_file}'`"",
        "run-shell `"$CALL set -g '@dcf' '#{d:current_file}'`"",
        'set -g @after on') | Out-Null
    Wait-Until { (Opt '@after') -eq 'on' } 15000 | Out-Null
    Check ((Opt '@cf') -eq $CONF) "#{current_file} is the config (tmux)" "@cf reads [$(Opt '@cf')], want [$CONF]"
    Check ((Opt '@dcf') -eq (Split-Path $CONF)) "#{d:current_file} is its directory (tmux)" "@dcf reads [$(Opt '@dcf')]"
    $RFILE = Join-Path $TMP "runtime.conf"
    @('split-window', "run-shell `"$CALL set -g '@rcf' '#{current_file}'`"") -join "`n" |
        Set-Content -Path $RFILE -Encoding ASCII
    P source-file $RFILE | Out-Null
    Check ((Opt '@rcf') -eq $RFILE) "a runtime source-file's command has it too (tmux)" "@rcf reads [$(Opt '@rcf')], want [$RFILE]"
    $other = ((P display-message -p 'cf=#{current_file}') -join '').Trim()
    Check ($other -eq 'cf=') "a command from outside the files has none" "display-message reads [$other]"

    # The shared server (#798) runs no source job yet, so it reads the config
    # as before: its options apply wherever they stand and its run-shell
    # starts in place. Keeping the lines after the first command for a job
    # that never runs would lose them.
    Write-Head "P. the shared server still applies the lines after the config's first command"
    $env:PSMUX_SERVER_MODE = "shared"
    try {
        Start-WithConfig @('set -g @before on', 'split-window', 'set -g @after on',
            "run-shell `"$CALL set -g '@ran' on`"") | Out-Null
        Check (Wait-Until { (Opt '@before') -eq 'on' } 8000) "the session started" "@before reads [$(Opt '@before')]"
        Check ((Opt '@after') -eq 'on') "the option after the first command applied" "@after reads [$(Opt '@after')]"
        Check (Wait-Until { (Opt '@ran') -eq 'on' } 8000) "the run-shell after it ran" "@ran reads [$(Opt '@ran')]"
    } finally {
        Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    }
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
