# Issue #847: source-file with more than one path.
#
# tmux's source-file takes one or more paths, each of which may be a glob,
# and reads them in order (tmux.1:1631 to :1656, cmd-source-file.c:42,
# :203 to :242 and :126 to :128). A path that cannot be read is reported and
# the others are still read; the client then exits 1 unless -q is given
# (cmd-source-file.c:221 to :236, :86 to :90).
#
# psmux read one path, and which one depended on the route: the CLI kept the
# last, the server's dispatcher (hooks, key bindings) and control mode the
# first, and a `source-file a b` line in a file was read as the single path
# `a b`, which cannot be opened. Control mode also took its first argument as
# the path when it was a flag, so `source-file -q f` read a file named `-q`.
#
# Every expected value below is what tmux 3.7c printed for the same files on a
# fresh server (`tmux -L x -f /dev/null`).
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

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_sfp_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP, (Join-Path $TMP "dir with space"), (Join-Path $TMP "g") | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "sfp" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "sf"

Write-Host "`n=== Issue #847: source-file reads every path it is given ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
# An option never set reads "invalid option: NAME", so anything but the value
# fails the check.
function Opt($name) { ((P show-options -gv $name) -join '').Trim() }
function Opts([string[]]$names) { ($names | ForEach-Object { "$_=$(Opt $_)" }) -join ' ' }

# Run the CLI `source-file` with these arguments: its exit code, stdout and
# stderr. Each argument is quoted, so a path with a space stays one argument.
function Source([string[]]$argv, [string]$cwd = $TMP) {
    $out = Join-Path $TMP "out.txt"; $err = Join-Path $TMP "err.txt"
    $list = @("-L", $NS, "source-file") + @($argv | ForEach-Object { "`"$_`"" })
    $p = Start-Process -FilePath $PSMUX -ArgumentList $list -WorkingDirectory $cwd `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
    return @{
        Code = $p.ExitCode
        Out  = ((Get-Content $out -Raw -EA SilentlyContinue) + "").Trim()
        Err  = ((Get-Content $err -Raw -EA SilentlyContinue) + "").Trim()
    }
}

# Send control mode lines, then a display-message so the client waits for the
# server to finish the ones before it. Returns the reply lines.
function Control([string[]]$lines) {
    $input_text = (($lines + "display-message -p ctl-done") -join "`n") + "`n"
    return @($input_text | & $PSMUX -L $NS -C attach -t $S 2>&1 | ForEach-Object { "$_" })
}

function Conf($name, [string[]]$lines) {
    $path = Join-Path $TMP $name
    $lines | Set-Content -Path $path -Encoding ASCII
    return $path
}

function Reset-Session {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 500
}

$fa = Conf "a.conf" @('set -g @a on')
$fb = Conf "b.conf" @('set -g @b on')
# Both append, so the result shows that both ran and in which order.
$fxa = Conf "xa.conf" @('set -ga @x a')
$fxb = Conf "xb.conf" @('set -ga @x b')
$fsp = Conf "dir with space\s.conf" @('set -g @sp on')
Conf "g\1.conf" @('set -g @g1 on') | Out-Null
Conf "g\2.conf" @('set -g @g2 on') | Out-Null
$missing = Join-Path $TMP "missing.conf"

try {
    # ── A. two paths from the CLI ──
    Write-Head "A. psmux source-file a b"
    Reset-Session
    $r = Source @($fa, $fb)
    Check ($r.Code -eq 0) "exits 0 (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    $o = Opts @('@a', '@b')
    Check ($o -eq '@a=on @b=on') "both files ran (tmux)" "options read [$o]"

    # ── B. order ──
    Write-Head "B. the files run in the order given"
    Reset-Session
    Source @($fxa, $fxb) | Out-Null
    $x1 = Opt '@x'
    P set-option -gu '@x' | Out-Null
    Source @($fxb, $fxa) | Out-Null
    $x2 = Opt '@x'
    Check (($x1 -eq 'ab') -and ($x2 -eq 'ba')) "xa xb gives ab, xb xa gives ba (tmux)" "read [$x1] then [$x2]"

    # ── C. a missing path in the middle ──
    Write-Head "C. psmux source-file a missing b"
    Reset-Session
    $r = Source @($fa, $missing, $fb)
    Write-Info "exit $($r.Code), stderr [$($r.Err)]"
    Check ($r.Code -eq 1) "exits 1 (tmux)" "exit $($r.Code)"
    Check ($r.Err -match 'missing\.conf') "stderr names the missing file (tmux)" "stderr [$($r.Err)]"
    $o = Opts @('@a', '@b')
    Check ($o -eq '@a=on @b=on') "the files on both sides ran (tmux)" "options read [$o]"

    Write-Head "C2. two missing paths"
    Reset-Session
    $r = Source @((Join-Path $TMP 'missing1.conf'), (Join-Path $TMP 'missing2.conf'))
    Check (($r.Code -eq 1) -and ($r.Err -match 'missing1\.conf') -and ($r.Err -match 'missing2\.conf')) `
        "exit 1 and both are reported (tmux)" "exit $($r.Code), stderr [$($r.Err)]"

    # ── D. -q ──
    Write-Head "D. psmux source-file -q a missing b"
    Reset-Session
    $r = Source @('-q', $fa, $missing, $fb)
    Check (($r.Code -eq 0) -and ($r.Err -eq '')) "exit 0, nothing on stderr (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    $o = Opts @('@a', '@b')
    Check ($o -eq '@a=on @b=on') "both files ran (tmux)" "options read [$o]"

    # ── E. a glob and a plain path ──
    Write-Head "E. psmux source-file g/*.conf b"
    Reset-Session
    $r = Source @((Join-Path $TMP 'g\*.conf'), $fb)
    $o = Opts @('@g1', '@g2', '@b')
    Check (($r.Code -eq 0) -and ($o -eq '@g1=on @g2=on @b=on')) "both matches and b ran (tmux)" "exit $($r.Code), options read [$o]"

    # ── F. relative paths ──
    Write-Head "F. relative paths, from the client's directory"
    Reset-Session
    $r = Source @('a.conf', 'b.conf') $TMP
    $o = Opts @('@a', '@b')
    Check (($r.Code -eq 0) -and ($o -eq '@a=on @b=on')) "both files ran (tmux)" "exit $($r.Code), stderr [$($r.Err)], options read [$o]"

    # ── G. a line in a file ──
    Write-Head "G. a source-file line with two paths"
    $fG = Conf "g.conf" @("source-file $fa `"$fsp`"")
    Reset-Session
    $r = Source @($fG)
    $o = Opts @('@a', '@sp')
    Check (($r.Code -eq 0) -and ($o -eq '@a=on @sp=on')) "both files ran, the quoted path with a space is one path (tmux)" "exit $($r.Code), stderr [$($r.Err)], options read [$o]"

    Write-Head "G2. a missing path on a line, between two"
    $fG2 = Conf "g2.conf" @("source-file $fa $missing $fb", 'set -g @after on')
    Reset-Session
    $r = Source @($fG2)
    Write-Info "exit $($r.Code), stderr [$($r.Err)]"
    Check (($r.Code -eq 1) -and ($r.Err -match 'missing\.conf')) "exit 1 and the missing file is named (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    $o = Opts @('@a', '@b', '@after')
    Check ($o -eq '@a=on @b=on @after=on') "the other files and the next line ran (tmux)" "options read [$o]"

    # ── H. an unquoted path with spaces, written before paths were split ──
    # psmux only: tmux splits this line into words. The line read this one file
    # before, so it keeps doing so while the whole text names a file.
    Write-Head "H. an unquoted path with spaces that names one file (kept as it was)"
    $fH = Conf "h.conf" @("source-file $fsp")
    Reset-Session
    $r = Source @($fH)
    Check (($r.Code -eq 0) -and ((Opt '@sp') -eq 'on')) "the file ran" "exit $($r.Code), stderr [$($r.Err)], @sp [$(Opt '@sp')]"

    # ── I. a hook (the server's dispatcher) ──
    Write-Head "I. source-file a b from a hook"
    Reset-Session
    P set-hook -g after-rename-window "source-file `"$fa`" `"$fb`"" | Out-Null
    P rename-window -t "${S}:0" trigger | Out-Null
    $o = ''
    for ($i = 0; $i -lt 30; $i++) {
        $o = Opts @('@a', '@b')
        if ($o -eq '@a=on @b=on') { break }
        Start-Sleep -Milliseconds 100
    }
    Check ($o -eq '@a=on @b=on') "both files ran (tmux)" "options read [$o]"

    # ── J. control mode ──
    Write-Head "J. control mode"
    Reset-Session
    Control @("source-file $fa $fb") | Out-Null
    $o = Opts @('@a', '@b')
    Check ($o -eq '@a=on @b=on') "source-file a b ran both (tmux)" "options read [$o]"
    Reset-Session
    Control @("source-file -q $missing $fa") | Out-Null
    Check ((Opt '@a') -eq 'on') "-q is a flag, so a missing first path is skipped (tmux)" "@a reads [$(Opt '@a')]"

    # ── K. no path ──
    Write-Head "K. source-file with no path"
    Reset-Session
    $r = Source @()
    Check (($r.Code -eq 1) -and ($r.Err -match 'too few arguments')) "CLI: exit 1, too few arguments (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    $reply = Control @("source-file")
    $err = @($reply | Where-Object { $_ -match '^%error ' })
    $msg = @($reply | Where-Object { $_ -match 'too few arguments' })
    Check (($err.Count -ge 1) -and ($msg.Count -ge 1)) "control mode: %error with the message" "reply [$($reply -join ' | ')]"

    # ── L. -n ──
    Write-Head "L. source-file -n with two paths runs nothing"
    Reset-Session
    $r = Source @('-n', $fa, $fb)
    Check ($r.Code -eq 0) "exits 0 (tmux)" "exit $($r.Code), stderr [$($r.Err)]"
    Check (((Opt '@a') -ne 'on') -and ((Opt '@b') -ne 'on')) "neither file ran (tmux)" "options read [$(Opts @('@a', '@b'))]"
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
