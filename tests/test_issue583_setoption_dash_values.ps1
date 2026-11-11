# Issue #583: set-option parsed flags PAST the option name, so a
# dash-leading VALUE was consumed as a flag instead of stored. Worst case
# was silent data loss: `set -t S @k -u` routed to the unset path and
# deleted the key at rc 0. There was also no `--` end-of-options escape
# (`--` itself was rejected as an unknown flag), and set-hook had the same
# silent-deletion shape. tmux (getopt) stops flag parsing at the option
# name, so everything after it is a literal value.
#
# The fix applies positional discipline in the CLI guard (main.rs), the
# server set-option parser, and the server set-hook parser: flags parse
# only before the first positional, and `--` ends option parsing entirely
# (same shape as the #562 send-keys fix).
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "t583e2e"
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }

# Run psmux with an exact argv (no shell re-tokenizing) and capture rc/out/err.
function Invoke-Psmux([string[]]$ArgList) {
    $so = Join-Path $env:TEMP "t583_out.txt"
    $se = Join-Path $env:TEMP "t583_err.txt"
    $p = Start-Process -FilePath $PSMUX -ArgumentList $ArgList -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $so -RedirectStandardError $se
    [pscustomobject]@{
        rc  = $p.ExitCode
        out = "$(Get-Content $so -Raw -ErrorAction SilentlyContinue)".Trim()
        err = "$(Get-Content $se -Raw -ErrorAction SilentlyContinue)".Trim()
    }
}
function Read-Opt($name) { (Invoke-Psmux @('show-options','-qv','-t',$SESSION,$name)).out }

$env:PSMUX_NO_WARM = "1"
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "session creation failed"; exit 1 }

Write-Host "`n=== Issue #583: dash-leading values in set-option / set-hook ===" -ForegroundColor Cyan

# --- Arm 1: control row still works ---
Write-Host "[Arm 1] control: plain value round trip" -ForegroundColor Yellow
$r = Invoke-Psmux @('set-option','-t',$SESSION,'@k','keepme')
if ($r.rc -eq 0 -and (Read-Opt '@k') -eq 'keepme') { Write-Pass "@k keepme stored at rc 0" }
else { Write-Fail "control row broken (rc=$($r.rc), value=[$(Read-Opt '@k')])" }

# --- Arm 2: `-u` in the VALUE position is data, not the unset flag ---
Write-Host "[Arm 2] dash-leading values are stored literally" -ForegroundColor Yellow
$r = Invoke-Psmux @('set-option','-t',$SESSION,'@k','-u')
$v = Read-Opt '@k'
if ($r.rc -eq 0 -and $v -eq '-u') { Write-Pass "value -u stored literally at rc 0 (was: silent key deletion)" }
else { Write-Fail "value -u: rc=$($r.rc) value=[$v] err=[$($r.err)]" }

$r = Invoke-Psmux @('set-option','-t',$SESSION,'@k','-x')
$v = Read-Opt '@k'
if ($r.rc -eq 0 -and $v -eq '-x') { Write-Pass "value -x stored literally at rc 0 (was: unknown flag rc 1)" }
else { Write-Fail "value -x: rc=$($r.rc) value=[$v] err=[$($r.err)]" }

$r = Invoke-Psmux @('set-option','-t',$SESSION,'status-left','-u')
$v = Read-Opt 'status-left'
if ($r.rc -eq 0 -and $v -eq '-u') { Write-Pass "built-in option stores dash-leading value too" }
else { Write-Fail "status-left -u: rc=$($r.rc) value=[$v]" }
Invoke-Psmux @('set-option','-t',$SESSION,'-u','status-left') | Out-Null

# --- Arm 3: `--` end-of-options ---
Write-Host "[Arm 3] -- end-of-options" -ForegroundColor Yellow
$r = Invoke-Psmux @('set-option','-t',$SESSION,'--','@k','-u')
$v = Read-Opt '@k'
if ($r.rc -eq 0 -and $v -eq '-u') { Write-Pass "set -- @k -u stores -u at rc 0 (was: unknown flag -- rc 1)" }
else { Write-Fail "set -- form: rc=$($r.rc) value=[$v] err=[$($r.err)]" }

$r = Invoke-Psmux @('show-options','-qv','-t',$SESSION,'--','@k')
if ($r.rc -eq 0 -and $r.out -eq '-u') { Write-Pass "show -qv -- @k prints the value at rc 0" }
else { Write-Fail "show -- form: rc=$($r.rc) out=[$($r.out)] err=[$($r.err)]" }

# --- Arm 4: the unset flag itself still unsets, and unknown flags still error ---
Write-Host "[Arm 4] flag region semantics preserved" -ForegroundColor Yellow
$r = Invoke-Psmux @('set-option','-t',$SESSION,'-u','@k')
$v = Read-Opt '@k'
if ($r.rc -eq 0 -and $v -eq '') { Write-Pass "set -u @k still unsets" }
else { Write-Fail "unset regressed: rc=$($r.rc) value=[$v]" }

$r = Invoke-Psmux @('set-option','-t',$SESSION,'-x','@k','v')
if ($r.rc -eq 1 -and $r.err -match 'unknown flag -x') { Write-Pass "unknown flag before the name still rc 1" }
else { Write-Fail "unknown-flag guard regressed: rc=$($r.rc) err=[$($r.err)]" }

$r = Invoke-Psmux @('set-option','-gq','@z583','zz')
if ($r.rc -eq 0 -and (Invoke-Psmux @('show-options','-qv','-t',$SESSION,'@z583')).out -eq 'zz') {
    Write-Pass "combined flag tokens (-gq) still parse in the flag region"
} else { Write-Fail "combined flags regressed (rc=$($r.rc))" }
Invoke-Psmux @('set-option','-gu','@z583') | Out-Null

# --- Arm 5: set-hook no longer silently deletes on a dash-leading command ---
Write-Host "[Arm 5] set-hook positional discipline" -ForegroundColor Yellow
Invoke-Psmux @('set-hook','-t',$SESSION,'after-new-window','display-message HOOK583') | Out-Null
$r = Invoke-Psmux @('set-hook','-t',$SESSION,'after-new-window','-u')
$hooks = (Invoke-Psmux @('show-hooks','-t',$SESSION)).out
if ($r.rc -eq 0 -and $hooks -match 'after-new-window' -and $hooks -notmatch 'no hooks') {
    Write-Pass "hook survives a dash-leading command token (was: silent deletion)"
} else { Write-Fail "set-hook name -u: rc=$($r.rc) hooks=[$hooks]" }

$r = Invoke-Psmux @('set-hook','-t',$SESSION,'-u','after-new-window')
$hooks = (Invoke-Psmux @('show-hooks','-t',$SESSION)).out
if ($r.rc -eq 0 -and $hooks -notmatch 'after-new-window') { Write-Pass "set-hook -u <name> still unsets" }
else { Write-Fail "hook unset regressed: rc=$($r.rc) hooks=[$hooks]" }

# --- Arm 6: #580 pane scope unaffected ---
Write-Host "[Arm 6] pane-scope (-p) round trip still works" -ForegroundColor Yellow
$r = Invoke-Psmux @('set-option','-p','-t',"${SESSION}:0.0",'remain-on-exit','failed')
$shown = (Invoke-Psmux @('show-options','-p','-t',"${SESSION}:0.0")).out
if ($r.rc -eq 0 -and $shown -match 'remain-on-exit failed') { Write-Pass "set -p / show -p round trip intact" }
else { Write-Fail "pane scope regressed: rc=$($r.rc) shown=[$shown]" }

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
