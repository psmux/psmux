# Issue #829: a pane spawned by a one-shot CLI command gets the PATH of the
# client that ran the command, not the PATH of the client that started the
# server (tmux spawn.c: an unattached client's PATH replaces the session's).
# Only PATH travels: another variable of the caller is not inherited, and the
# server's global PATH is not changed.
#
# Uses $env:PSMUX_TEST_BIN when set, else the psmux on PATH. Runs in its own
# -L namespace with a private data dir; cleans up only that namespace.
# Exit code = number of failures.

$ErrorActionPreference = 'Continue'
$Bin = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux).Source }
$ns = "t829_$PID"
$d = Join-Path $env:TEMP "psmux_t829_$PID"
New-Item -ItemType Directory -Force "$d\data" | Out-Null
Set-Content "$d\empty.conf" ''
$saved = @{ DATA = $env:PSMUX_DATA_DIR; CONF = $env:PSMUX_CONFIG_FILE; WARM = $env:PSMUX_NO_WARM; SHELL = $env:SHELL; PATH = $env:PATH }
$env:PSMUX_DATA_DIR = "$d\data"
$env:PSMUX_CONFIG_FILE = "$d\empty.conf"
Remove-Item Env:PSMUX_NO_WARM -ErrorAction SilentlyContinue   # keep the warm pool on
Remove-Item Env:SHELL -ErrorAction SilentlyContinue
Remove-Item Env:BH_MARK -ErrorAction SilentlyContinue

$fail = 0; $pass = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:pass++; Write-Host "PASS $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "FAIL $name  $detail" -ForegroundColor Red }
}
function P { & $Bin -L $ns @args }

Set-Content "$d\probe.ps1" @'
param($Out)
"pathmarker=$([bool]($env:PATH -like '*bh-path-marker*')) BH_MARK=[$env:BH_MARK]" | Set-Content $Out
Start-Sleep 120
'@
function Probe($name) { @('pwsh', '-NoProfile', '-File', "$d\probe.ps1", "$d\$name.txt") }
function ReadProbe($name) {
    $f = "$d\$name.txt"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $f); $i++) { Start-Sleep -Milliseconds 250 }
    if (Test-Path $f) { (Get-Content $f -Raw).Trim() } else { '<no output>' }
}
$base = $env:PATH
$marked = "C:\bh-path-marker;$base"

try {
    # Server started by a client WITHOUT the marker.
    P new-session -d -s s1 -n first @(Probe 'initial')
    Start-Sleep 2
    Check 'initial pane has the starting client PATH' ((ReadProbe 'initial') -eq 'pathmarker=False BH_MARK=[]') (ReadProbe 'initial')

    # A later client WITH the marker.
    $env:PATH = $marked; $env:BH_MARK = 'client2'
    P new-window -t s1: @(Probe 'nw')
    P split-window -t s1: @(Probe 'sw')
    P new-pane -t s1: @(Probe 'np')
    # A command list goes to the server as one line; each command's shell
    # command is passed as one string there.
    P new-window -t s1: -n listwin ((Probe 'list1') -join ' ') `; split-window ((Probe 'list2') -join ' ')
    P new-window -t s1: -e 'PATH=C:\explicit-only' @(Probe 'explicit')
    P new-window -d -t s1: -n shellwin
    $env:PATH = $base; Remove-Item Env:BH_MARK

    Check 'new-window gets caller PATH, not BH_MARK' ((ReadProbe 'nw') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'nw')
    Check 'split-window gets caller PATH' ((ReadProbe 'sw') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'sw')
    Check 'new-pane gets caller PATH' ((ReadProbe 'np') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'np')
    Check 'command list new-window gets caller PATH' ((ReadProbe 'list1') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'list1')
    Check 'command list split-window gets caller PATH' ((ReadProbe 'list2') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'list2')
    Check 'explicit -e PATH is kept' ((ReadProbe 'explicit') -eq 'pathmarker=False BH_MARK=[]') (ReadProbe 'explicit')

    # Default shell window (warm pool on): its shell must also have the PATH.
    Start-Sleep 3
    P send-keys -t s1:shellwin '"shellpath=$([bool]($env:PATH -like ''*bh-path-marker*''))"' Enter
    $got = ''
    for ($i = 0; $i -lt 20 -and -not $got; $i++) {
        Start-Sleep -Milliseconds 500
        $got = (P capture-pane -p -t s1:shellwin) | Select-String '^shellpath=' | Select-Object -Last 1
    }
    Check 'default shell new-window gets caller PATH' ("$got" -eq 'shellpath=True') "got [$got]"

    Check 'global PATH unchanged' (-not ((P show-environment -g PATH) -like '*bh-path-marker*')) ''

    # respawn-pane / respawn-window from the marker client.
    $env:PATH = $marked
    & $Bin -L $ns respawn-pane -k -t s1:first -- @(Probe 'rp')   # a function would eat the --
    $env:PATH = $base
    Check 'respawn-pane gets caller PATH' ((ReadProbe 'rp') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'rp')
    $env:PATH = $marked
    & $Bin -L $ns respawn-window -k -t s1:listwin -- @(Probe 'rw')
    $env:PATH = $base
    Check 'respawn-window gets caller PATH' ((ReadProbe 'rw') -eq 'pathmarker=True BH_MARK=[]') (ReadProbe 'rw')

    # Reverse: a client without the marker after the marked client.
    P new-window -t s1: @(Probe 'rev')
    Check 'caller without marker gets its own PATH' ((ReadProbe 'rev') -eq 'pathmarker=False BH_MARK=[]') (ReadProbe 'rev')

    # Same PATH as the server: the warm pool still serves the pane (its shell
    # was started before the command ran).
    Start-Sleep 4
    $t0 = Get-Date
    $pp = P new-window -d -t s1: -P -F '#{pane_pid}'
    $proc = Get-Process -Id ([int]"$pp".Trim()) -ErrorAction SilentlyContinue
    Check 'same PATH new-window uses a warm spare' ($proc -and $proc.StartTime -lt $t0.AddMilliseconds(-200)) "pid=$pp start=$($proc.StartTime.ToString('HH:mm:ss.fff')) cmd=$($t0.ToString('HH:mm:ss.fff'))"
}
finally {
    P kill-server 2>$null | Out-Null
    $env:PATH = $saved.PATH
    $env:PSMUX_DATA_DIR = $saved.DATA; $env:PSMUX_CONFIG_FILE = $saved.CONF
    if ($saved.WARM) { $env:PSMUX_NO_WARM = $saved.WARM }
    if ($saved.SHELL) { $env:SHELL = $saved.SHELL }
    Start-Sleep 1
    Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
}
Write-Host "passed=$pass failed=$fail"
exit $fail
