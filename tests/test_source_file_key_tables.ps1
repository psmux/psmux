# source-file and the key tables.
#
# tmux's source-file only runs the file's commands (cmd-source-file.c); it
# never resets the key tables. psmux cleared every table and re-seeded the
# defaults on EVERY runtime source-file. Measured against tmux 3.4 with the
# same steps, before this change:
#
#   step                                          tmux 3.4   psmux
#   bind -n F2 (CLI), source-file other.conf      F2 kept    F2 lost
#   source-file p1.conf, then p2.conf (plugins)    both kept  only p2
#   unbind-key -a, then list root F2               kept       lost (all tables)
#
# psmux keeps one deliberate difference (#195, 266d414): source-file of the
# server's STARTUP config is a reload and starts from the defaults, so a
# config that stops saying `unbind-key -a` gets the prefix defaults back. Any
# other file never resets.
#
# GROUND TRUTH: list-keys.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }
Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null; $env:PSMUX_SESSION = $null; $env:PSMUX_PANE = $null
$env:TMUX = $null; $env:TMUX_PANE = $null

$ownData = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path ([IO.Path]::GetTempPath()) ("psmux_sfkt_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownData = $true
}
$NS  = "sfkt-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$DIR = Join-Path ([IO.Path]::GetTempPath()) ("psmux_sfkt_files_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $DIR | Out-Null
$MAIN = Join-Path $DIR "main.conf"
function Conf($name, $text) { $p = Join-Path $DIR $name; [IO.File]::WriteAllText($p, $text + "`n"); $p }
function P { & $PSMUX -L $NS @args 2>&1 }
function Has($table, $key) {
    [bool]((P list-keys) | % { ($_ -replace '\s+', ' ') + ' ' } | Select-String -SimpleMatch "-T $table $key ")
}
function Count($table) { @((P list-keys) | % { ($_ -replace '\s+', ' ') } | Select-String -SimpleMatch "-T $table ").Count }

try {
    [void](Conf "main.conf" "bind x display main")
    $env:PSMUX_CONFIG_FILE = $MAIN
    P new-session -d -s s -x 100 -y 20 | Out-Null
    $env:PSMUX_CONFIG_FILE = $null
    Start-Sleep 3
    $defaults = Count prefix

    Write-Head "a file that is not the startup config only adds"
    P bind -n F2 display cli | Out-Null
    P source-file (Conf "other.conf" "bind -n F3 display other") | Out-Null
    Check ((Has root F2) -and (Has root F3)) "CLI binding survives source-file of another file" "F2=$(Has root F2) F3=$(Has root F3)"
    P source-file (Conf "p1.conf" "bind -n F4 display p1") | Out-Null
    P source-file (Conf "p2.conf" "bind -n F5 display p2") | Out-Null
    Check ((Has root F4) -and (Has root F5)) "files sourced one after another all keep their bindings" "F4=$(Has root F4) F5=$(Has root F5)"
    $ua = Conf "ua.conf" "unbind-key -a`nbind z display zed"
    P source-file $ua | Out-Null
    $c1 = Count prefix
    [void](Conf "ua.conf" "#unbind-key -a`nbind z display zed")
    P source-file $ua | Out-Null
    Check ((Count prefix) -eq $c1) "unbind-key -a commented out in a NON-startup file: defaults stay unbound (tmux)" "prefix $c1 -> $(Count prefix)"
    Check (Has root F2) "unbind-key -a left the root table alone" "root F2 gone"

    Write-Head "the startup config is a reload (#195)"
    [void](Conf "main.conf" "unbind-key -a`nset -g prefix C-a`nbind-key C-a send-prefix")
    P source-file $MAIN | Out-Null
    $after = Count prefix
    Check ($after -eq 1) "startup config with unbind-key -a: one prefix binding" "prefix=$after"
    [void](Conf "main.conf" "#unbind-key -a`nset -g prefix C-a`nbind-key C-a send-prefix")
    # Spelled differently: upper case and forward slashes name the same file.
    P source-file ($MAIN.ToUpper().Replace('\', '/')) | Out-Null
    $back = Count prefix
    Check ($back -ge $defaults) "commented out and reloaded: the prefix defaults come back ($back)" "prefix=$back, defaults were $defaults"
    Check (-not (Has root F4)) "the reload started from the defaults" "root F4 survived a startup config reload"

    Write-Head "unbind-key -a tables"
    P bind -n F6 display r | Out-Null
    P unbind-key -a | Out-Null
    Check ((Count prefix) -eq 0 -and (Has root F6) -and (Has root WheelUpPane)) "unbind-key -a: prefix only" "prefix=$(Count prefix) F6=$(Has root F6)"
    P unbind-key -a -T root | Out-Null
    Check (-not (Has root F6) -and -not (Has root WheelUpPane)) "unbind-key -a -T root clears root" "root still has F6=$(Has root F6)"
    P unbind-key -a -T copy-mode | Out-Null
    Check ((Count copy-mode) -eq 0 -and (Count copy-mode-vi) -gt 0) "unbind-key -a -T copy-mode clears copy-mode only" "copy-mode=$(Count copy-mode) copy-mode-vi=$(Count copy-mode-vi)"
} finally {
    P kill-server | Out-Null
    Start-Sleep 1
    Remove-Item -Recurse -Force $DIR -EA SilentlyContinue
    if ($ownData) { Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue }
}
Write-Host "`nResults: $script:TestsPassed passed, $script:TestsFailed failed"
exit $(if ($script:TestsFailed -eq 0) { 0 } else { 1 })
