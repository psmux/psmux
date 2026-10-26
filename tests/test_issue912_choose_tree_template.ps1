# Issue #912: choose-tree ignored its template argument, so Enter always did
# the built-in switch and `choose-tree -Zs "display-message 'picked %%'"`
# never ran.
#
# tmux 3.7c: window_tree_init keeps `args_string(args, 0)` as the command and
# defaults to `switch-client -Zt '%%'` (window-tree.c). Enter and the jump keys
# run it through mode_tree_run_command with `%%` (first only) and `%1` set to
# the row's target from window_tree_get_target: `=s:` for a session,
# `=s:idx.` for a window and `=s:idx.%id` for a pane. The client switches only
# when the template says so.
#
# Live: a real attached client, keys through WriteConsoleInput
# (tests/injector.cs). Each template records the target it was given in a
# user option, read back through the CLI, and list-clients shows where the
# client is. Isolated: -L, and USERPROFILE, PSMUX_DATA_DIR and
# PSMUX_CONFIG_FILE point into a temp dir. PSMUX_EXE picks the binary
# (defaults to target\release).

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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_912_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
. "$PSScriptRoot\isolated_config.ps1" -Lines @(
    "bind T choose-tree -Zs `"set -g @picked '%%'`"",
    "bind U choose-tree -Zw `"set -g @picked '%%'`"",
    "bind M choose-tree -s `"set -g @one '%%' ; set -g @two '%1'`"",
    "bind E choose-tree -s `"`"",
    "bind V choose-tree -Zs `"switch-client -t '%%'`"",
    "bind W choose-window `"set -g @picked '%%'`"",
    "bind X choose-tree -Zw `"switch-client -t '%%'`""
)
$NS = "t912_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$A = "a912"
$B = "b912"
function P { & $PSMUX -L $NS @args 2>&1 }

Write-Host "`n=== Issue #912: choose-tree [template] ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((P -V) -join ' ')"

$toolDir = Join-Path $TempHome "tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $toolDir "injector.exe"
& $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $INJ)) { Write-Fail "injector did not compile"; exit 1 }

function Start-Client($session) {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach", "-t", $session -PassThru
    Start-Sleep -Seconds 4
    return $p
}
function Stop-Client($p) {
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 700
}
function Send-Keys($p, $keys) {
    & $INJ $p.Id $keys | Out-Null
    Start-Sleep -Milliseconds 1200
}
function Opt($name) {
    $v = P show-options -gv $name
    if ($LASTEXITCODE -ne 0) { return "<unset>" }
    return (($v | Out-String).Trim())
}
function Client-Session { return ((P list-clients -F '#{client_session}') | Out-String).Trim() }
function Reset-Opts { foreach ($o in '@picked', '@one', '@two') { P set-option -gu $o | Out-Null } }

# One case: a fresh client on $A, keys in, then the option and where the
# client sits.
function Check-Case($name, $keys, $opt, $wantOpt, $wantSession) {
    Reset-Opts
    $c = Start-Client $A
    if ($keys -is [scriptblock]) { & $keys $c } else { Send-Keys $c $keys }
    Start-Sleep -Milliseconds 800
    $got = Opt $opt
    $where = Client-Session
    if ($got -eq $wantOpt) { Write-Pass "$name ran the template with $opt=$got" }
    else { Write-Fail "$name expected $opt=$wantOpt, got $got" }
    if ($where -eq $wantSession) { Write-Pass "$name left the client in $where" }
    else { Write-Fail "$name expected the client in $wantSession, got '$where'" }
    Stop-Client $c
}

try {
    P new-session -d -s $A -x 120 -y 30 | Out-Null
    P new-session -d -s $B -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t $A | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }
    $paneA = ((P display-message -t "${A}:0.0" -p '#{pane_id}') | Out-String).Trim()

    Write-Host "`n--- the session list (choose-tree -s) ---" -ForegroundColor Cyan
    # Rows sorted by name: a912, b912. Down moves to b912.
    Check-Case "prefix T, b912, Enter" "^b{SLEEP:500}T{SLEEP:1500}{DOWN}{SLEEP:400}{ENTER}" '@picked' "=${B}:" $A
    Check-Case "prefix T, a912 (own session), Enter" "^b{SLEEP:500}T{SLEEP:1500}{ENTER}" '@picked' "=${A}:" $A
    Check-Case "%% and %1 in a two command template" "^b{SLEEP:500}M{SLEEP:1500}{DOWN}{SLEEP:400}{ENTER}" '@two' "=${B}:" $A
    $one = Opt '@one'
    if ($one -eq "=${B}:") { Write-Pass "the first command of the template ran too (@one=$one)" } else { Write-Fail "the first command did not run, @one=$one" }
    Check-Case "an empty template runs nothing and does not switch" "^b{SLEEP:500}E{SLEEP:1500}{DOWN}{SLEEP:400}{ENTER}" '@picked' "<unset>" $A

    Write-Host "`n--- the tree (choose-tree -w) ---" -ForegroundColor Cyan
    # Windows start collapsed: a912, a912:0, b912, b912:0, and the cursor
    # opens on a912's active window.
    Check-Case "prefix U, own window, Enter" "^b{SLEEP:500}U{SLEEP:1500}{ENTER}" '@picked' "=${A}:0." $A
    Check-Case "prefix U, own session row, Enter" "^b{SLEEP:500}U{SLEEP:1500}{UP}{SLEEP:400}{ENTER}" '@picked' "=${A}:" $A
    Check-Case "prefix U, the other session's window, Enter" "^b{SLEEP:500}U{SLEEP:1500}{DOWN}{SLEEP:300}{DOWN}{SLEEP:400}{ENTER}" '@picked' "=${B}:0." $A
    Check-Case "prefix U, a pane row, Enter" "^b{SLEEP:500}U{SLEEP:1500}{RIGHT}{SLEEP:300}{DOWN}{SLEEP:400}{ENTER}" '@picked' "=${A}:0.$paneA" $A
    Check-Case "prefix U, jump key 2 (the b912 row)" "^b{SLEEP:500}U{SLEEP:1500}2" '@picked' "=${B}:" $A
    Check-Case "choose-window alias with a template" "^b{SLEEP:500}W{SLEEP:1500}{ENTER}" '@picked' "=${A}:0." $A

    Write-Host "`n--- the command prompt and the CLI ---" -ForegroundColor Cyan
    Check-Case ":choose-tree -s with a template" "^b{SLEEP:500}:{SLEEP:600}choose-tree -s `"set -g @picked '%%'`"{ENTER}{SLEEP:1500}{DOWN}{SLEEP:400}{ENTER}" '@picked' "=${B}:" $A
    Check-Case "psmux choose-tree -s with a template" { param($c) P choose-tree -s -t $A "set -g @picked '%%'" | Out-Null; Start-Sleep -Milliseconds 1500; Send-Keys $c "{DOWN}{SLEEP:400}{ENTER}" } '@picked' "=${B}:" $A
    Check-Case "a hook running choose-tree -w with a template" { param($c) P set-hook -t $A after-new-window "choose-tree -w `"set -g @picked '%%'`"" | Out-Null; P new-window -d -t $A | Out-Null; Start-Sleep -Milliseconds 1500; Send-Keys $c "{ENTER}" } '@picked' "=${A}:0." $A
    P set-hook -u -t $A after-new-window | Out-Null
    P kill-window -t "${A}:1" | Out-Null

    Write-Host "`n--- a template that switches, and no template ---" -ForegroundColor Cyan
    Check-Case "switch-client -t '%%' template" "^b{SLEEP:500}V{SLEEP:1500}{DOWN}{SLEEP:400}{ENTER}{SLEEP:1500}" '@picked' "<unset>" $B
    Check-Case "switch-client -t '%%' template on a window row" "^b{SLEEP:500}X{SLEEP:1500}{DOWN}{SLEEP:300}{DOWN}{SLEEP:400}{ENTER}{SLEEP:1500}" '@picked' "<unset>" $B
    Check-Case "prefix s with no template still switches" "^b{SLEEP:500}s{SLEEP:1500}{DOWN}{SLEEP:400}{ENTER}{SLEEP:1500}" '@picked' "<unset>" $B
    Check-Case "prefix w with no template still switches" "^b{SLEEP:500}w{SLEEP:1500}{DOWN}{SLEEP:300}{DOWN}{SLEEP:400}{ENTER}{SLEEP:1500}" '@picked' "<unset>" $B

    Write-Host "`n--- Layer 2: the client still works after the choosers ---" -ForegroundColor Cyan
    $c = Start-Client $A
    Send-Keys $c "^b{SLEEP:500}T{SLEEP:1500}{ESC}"
    $alive = -not (Get-Process -Id $c.Id -EA SilentlyContinue).HasExited
    $n = ((P list-sessions -F '#{session_name}') | Measure-Object).Count
    if ($alive -and $n -eq 2) { Write-Pass "visible client alive after Esc, sessions=$n" } else { Write-Fail "client alive=$alive sessions=$n" }
    Stop-Client $c
}
finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed ===" -ForegroundColor $(if ($script:TestsFailed -eq 0) { 'Green' } else { 'Yellow' })
exit $script:TestsFailed
