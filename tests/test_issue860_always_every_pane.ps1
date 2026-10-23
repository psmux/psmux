# Issue #860 regression: `set -g extended-keys always` puts EVERY pane in
# extended key mode 1, as tmux does when it creates a screen (screen.c
# screen_reinit, screen-write.c screen_write_reset), whichever way psmux
# created the pane:
#
#   * the early boot pane, spawned before the config ran (command first pane
#     and default shell first pane)
#   * a second session in the same namespace (warm claim)
#   * new-window and split-window, which take a spare from the pool
#   * a pane created after `set -g extended-keys always` at run time: the
#     spare it takes was spawned while the option was still off
#   * the same under PSMUX_SERVER_MODE=shared
#
# And the tmux rule for a pane that already exists: a later `set` does not
# change its mode, only panes created afterwards take the new value.
#
# Every runtime check uses its own -L namespace and PSMUX_DATA_DIR, and
# nothing is stopped by name.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$TMP = Join-Path $env:TEMP "psmux_t860a_$PID"
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $TMP "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
$savedMode = $env:PSMUX_SERVER_MODE

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }

$always = Join-Path $TMP "always.conf"; 'set -g extended-keys always' | Set-Content $always -Encoding ASCII
$empty = Join-Path $TMP "empty.conf"; '' | Set-Content $empty -Encoding ASCII
$on = Join-Path $TMP "on.conf"; 'set -g extended-keys on' | Set-Content $on -Encoding ASCII

function KeyMode($ns, $t) { (& $PSMUX -L $ns display-message -p -t $t '#{pane_key_mode}' 2>&1 | Out-String).Trim() }
function Expect($ns, $t, $want, $label) {
    $got = ''
    for ($i = 0; $i -lt 20; $i++) {
        $got = KeyMode $ns $t
        if ($got -eq $want) { break }
        Start-Sleep -Milliseconds 150
    }
    if ($got -eq $want) { Write-Pass "$label : $got" } else { Write-Fail "$label : [$got], want [$want]" }
}
function Wait-Session($ns, $s) {
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $ns has-session -t $s 2>$null; if ($LASTEXITCODE -eq 0) { return }; Start-Sleep -Milliseconds 150 }
}
function NewNs { "t860a_$($PID)_$(Get-Random)" }

$defaultPsmux = Get-Command psmux -EA SilentlyContinue
$before = if ($defaultPsmux) { (& $defaultPsmux.Source ls 2>&1 | Out-String) } else { "" }

foreach ($mode in @('', 'shared')) {
    if ($mode) { $env:PSMUX_SERVER_MODE = $mode } else { Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue }
    $tag = if ($mode) { 'shared' } else { 'default' }
    Write-Host "`n=== server mode: $tag ===" -ForegroundColor Cyan

    # 1. Command first pane, the early boot pane, then a second and third
    #    session (warm claim) and new-window / split-window (pool spares).
    $ns = NewNs
    Write-Host "[$tag] command first pane under -f always" -ForegroundColor Yellow
    & $PSMUX -L $ns -f $always new-session -d -s a -x 100 -y 30 -- cmd.exe /k echo ready 2>&1 | Out-Null
    Wait-Session $ns a
    Expect $ns a 'Ext 1' "[$tag] boot pane running a command"
    & $PSMUX -L $ns -f $always new-session -d -s b -x 100 -y 30 -- cmd.exe /k echo ready 2>&1 | Out-Null
    Wait-Session $ns b
    Expect $ns b 'Ext 1' "[$tag] second session running a command"
    & $PSMUX -L $ns -f $always new-session -d -s c -x 100 -y 30 2>&1 | Out-Null
    Wait-Session $ns c
    Expect $ns c 'Ext 1' "[$tag] third session, default shell"
    & $PSMUX -L $ns new-window -t a 2>&1 | Out-Null
    Expect $ns a 'Ext 1' "[$tag] new-window, default shell"
    & $PSMUX -L $ns new-window -t a -- cmd.exe /k echo ready 2>&1 | Out-Null
    Expect $ns a 'Ext 1' "[$tag] new-window running a command"
    & $PSMUX -L $ns split-window -t a 2>&1 | Out-Null
    Expect $ns a 'Ext 1' "[$tag] split-window"
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400

    # 2. Default shell first pane under always.
    $ns = NewNs
    Write-Host "[$tag] default shell first pane under -f always" -ForegroundColor Yellow
    & $PSMUX -L $ns -f $always new-session -d -s d -x 100 -y 30 2>&1 | Out-Null
    Wait-Session $ns d
    Expect $ns d 'Ext 1' "[$tag] boot pane, default shell"
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400

    # 3. A session after one whose config said `on` was killed (the order the
    #    #860 suite runs in).
    $ns = NewNs
    Write-Host "[$tag] always after an on session in the same namespace" -ForegroundColor Yellow
    & $PSMUX -L $ns -f $on new-session -d -s e -x 100 -y 30 -- cmd.exe /k echo ready 2>&1 | Out-Null
    Wait-Session $ns e
    Expect $ns e 'VT10x' "[$tag] on, app asked nothing"
    & $PSMUX -L $ns kill-session -t e 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX -L $ns -f $always new-session -d -s f -x 100 -y 30 -- cmd.exe /k echo ready 2>&1 | Out-Null
    Wait-Session $ns f
    Expect $ns f 'Ext 1' "[$tag] always after on"
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400

    # 4. Run time: the existing pane keeps its mode (tmux reads the option
    #    when it creates a screen), panes created afterwards take mode 1 even
    #    though the pool spare they use was spawned while the option was off.
    $ns = NewNs
    Write-Host "[$tag] set -g extended-keys always at run time" -ForegroundColor Yellow
    & $PSMUX -L $ns -f $empty new-session -d -s r -x 100 -y 30 2>&1 | Out-Null
    Wait-Session $ns r
    Start-Sleep -Milliseconds 1500
    Expect $ns 'r:0' 'VT10x' "[$tag] before the set"
    & $PSMUX -L $ns set -g extended-keys always 2>&1 | Out-Null
    Expect $ns 'r:0' 'VT10x' "[$tag] the existing pane keeps its mode (tmux)"
    & $PSMUX -L $ns new-window -t r 2>&1 | Out-Null
    Expect $ns 'r:1' 'Ext 1' "[$tag] new-window after the set"
    & $PSMUX -L $ns split-window -t 'r:1' 2>&1 | Out-Null
    Expect $ns 'r:1' 'Ext 1' "[$tag] split-window after the set"
    & $PSMUX -L $ns set -g extended-keys off 2>&1 | Out-Null
    & $PSMUX -L $ns new-window -t r 2>&1 | Out-Null
    Expect $ns 'r:2' 'VT10x' "[$tag] new-window after set back to off"
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
}
if ($savedMode) { $env:PSMUX_SERVER_MODE = $savedMode } else { Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue }

$after = if ($defaultPsmux) { (& $defaultPsmux.Source ls 2>&1 | Out-String) } else { "" }
if ($before -ne $after) { Write-Fail "the default namespace session list changed" }
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
exit $script:TestsFailed
