# Issue #627: an unqualified `%N` / `@N` target answered for the WRONG session.
#
# psmux runs one server per session and each allocates pane and window ids from
# its own counter, so `%1` and `@1` exist in every session at once. tmux makes
# these ids unique per server, which is what lets an unqualified `-t %N` name
# one specific pane there.
#
# Reported rig (four panes across two sessions):
#
#   alpha @1 %1 / alpha @2 %2 / alpha @2 %3 / beta @1 %1
#
# and `display-message -p -t <T> '#{session_name}/#{window_id}/#{pane_id}'`:
#
#   -t %3      ->  beta/@1/%1                        rc 0   (bug A)
#   -t @2      ->  ERROR: can't find window: @2       rc 0   (bug B, on STDOUT)
#   -t %9999   ->  psmux: can't find pane: %9999      rc 1   (already correct)
#
# Bug A: a bare `%N` has no session component, so the global -t parse leaves
# PSMUX_TARGET_SESSION unset and routing falls back to the most recently created
# session. The client already enumerated the owners of the id (#569) but used
# the list only to REFUSE when several sessions owned it; a single unambiguous
# owner was computed and thrown away. The command then reached a session that
# does not hold the id, and format::expand_format_for_pane_by_id falls back to
# expand_format (the ACTIVE pane) on a miss, so the answer looked like a real
# value at exit 0.
#
# Bug B: the validator had no arm for a bare `@N` at all, so nothing resolved or
# validated it. It went to the recency-picked session, whose FocusTargetTemp
# check answered `ERROR: can't find window: @2`, and the `display-message -p`
# client arm printed that reply verbatim with print!() and returned Ok(()).
# Hence the `ERROR:` prefix, which no exit-1 path in the client ever uses.
#
# What this suite pins:
#   * every row of the reported table, by exact text and exit code
#   * a unique owner wins for BOTH id kinds, and for `@N.%M` / `@N.<idx>` too
#   * the resolution is real routing, not just a lucky format expansion:
#     `send-keys -t %N` lands in the owning session's pane (proved by reading
#     the marker back out with capture-pane)
#   * a nonexistent `@N` exits 1 on STDERR with the client's `psmux: ` prefix,
#     and stdout stays empty
#   * several owners plus a routed session that owns none of them is refused
#     (exit 1, "ambiguous ... qualify as session:window.pane"), the #569 rule
#   * qualified forms (`alpha:@2`, `alpha:@2.%3`) still resolve
#   * a Win32 TUI section: one really attached window, addressed by its bare
#     `%N` from outside, verified through the CLI
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Section($m) { Write-Host ""; Write-Host $m -ForegroundColor Cyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

# Inherited session routing would aim these calls at somebody else's server.
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null
# Every new-session in a fresh namespace spawns a warm standby that outlives
# kill-session, so a suite that creates namespaces per run leaks one server per
# namespace. Nothing here depends on the warm pool (list_session_names_ns skips
# warm bases, so they can never be counted as owners of an id).
$env:PSMUX_NO_WARM      = '1'

$rig  = Join-Path $env:TEMP ("psmux627-" + [guid]::NewGuid().ToString('N').Substring(0,8))
$root = Join-Path $rig 'data'
New-Item -ItemType Directory -Force -Path $rig, $root | Out-Null
$env:PSMUX_DATA_DIR = $root
$conf = Join-Path $rig 'empty.conf'
Set-Content -Path $conf -Value '' -Encoding ascii

$NS  = 'bug627-' + [guid]::NewGuid().ToString('N').Substring(0,6)
$SA  = 'i627alpha'
$SB  = 'i627beta'
$SG  = 'i627gamma'
$FMT = '#{session_name}/#{window_id}/#{pane_id}'

function Run($argv) {
    $all = & $PSMUX @argv 2>&1
    $rc  = $LASTEXITCODE
    $so  = @(); $se = @()
    foreach ($r in $all) {
        if ($r -is [System.Management.Automation.ErrorRecord]) { $se += $r.ToString() }
        else { $so += ($r | Out-String).TrimEnd() }
    }
    return [pscustomobject]@{
        rc     = $rc
        stdout = (($so -join "`n") -replace "`r", '').Trim()
        stderr = (($se -join "`n") -replace "`r", '').Trim()
        all    = (((($all | Out-String) -replace "`r", '')).Trim())
    }
}
# Run the binary with the two streams kept APART. PowerShell cannot express
# this with redirection operators: `2>&1 1>$null` merges stderr into the
# success stream first and then throws both away, and `&`-invocation folds
# stderr into the output as ErrorRecords. #627 is partly about WHICH stream a
# diagnostic goes to, so the distinction has to be real.
function Split-Streams($argv) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $PSMUX
    foreach ($a in $argv) { $psi.ArgumentList.Add([string]$a) }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $o = $p.StandardOutput.ReadToEnd()
    $e = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    return [pscustomobject]@{
        rc     = $p.ExitCode
        stdout = ($o -replace "`r", '')
        stderr = ($e -replace "`r", '')
    }
}

function Px($argv) { return Run (@('-L', $NS, '-f', $conf) + $argv) }
function Disp($t) { return Px @('display-message', '-p', '-t', $t, $FMT) }

# ── rig ────────────────────────────────────────────────────────────────────
Write-Section "SETUP: two sessions, three panes in alpha, one in beta"
Px @('new-session',  '-d', '-s', $SA, '-n', 'w1')  | Out-Null
Start-Sleep -Milliseconds 700
Px @('new-window',   '-d', '-t', $SA, '-n', 'w2')  | Out-Null
Start-Sleep -Milliseconds 400
Px @('split-window', '-d', '-t', "${SA}:w2")       | Out-Null
Start-Sleep -Milliseconds 400
Px @('new-session',  '-d', '-s', $SB, '-n', 'b1')  | Out-Null
Start-Sleep -Milliseconds 700

$panes = Px @('list-panes', '-a', '-F', '#{session_name} #{window_id} #{pane_id}')
Write-Info "list-panes -a:`n$($panes.stdout)"
$rows = @($panes.stdout -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($rows -contains "$SA @2 %3" -and $rows -contains "$SB @1 %1") {
    Write-Pass "rig matches the report: %3 and @2 exist ONLY in $SA"
} else {
    Write-Fail "rig did not build as reported (got $($rows -join ' | ')) - later rows are meaningless"
}

# ── the reported table ─────────────────────────────────────────────────────
Write-Section "ROW 1: -t %3 resolves to its unique owner, not the newest session"
$r = Disp '%3'
if ($r.rc -eq 0 -and $r.stdout -eq "$SA/@2/%3") {
    Write-Pass "-t %3 -> $($r.stdout) rc=$($r.rc)"
} else {
    Write-Fail "-t %3 -> '$($r.stdout)' rc=$($r.rc), expected '$SA/@2/%3' rc=0"
}

Write-Section "ROW 2: -t @2 resolves to its unique owner"
$r = Disp '@2'
if ($r.rc -eq 0 -and $r.stdout -eq "$SA/@2/%2") {
    Write-Pass "-t @2 -> $($r.stdout) rc=$($r.rc)"
} else {
    Write-Fail "-t @2 -> '$($r.stdout)' rc=$($r.rc), expected '$SA/@2/%2' rc=0"
}
# The reported shape precisely: the error text used to arrive on STDOUT at rc 0.
$only = & $PSMUX -L $NS -f $conf display-message -p -t '@2' $FMT 2>$null
$onlyRc = $LASTEXITCODE
if ($onlyRc -eq 0 -and (($only | Out-String).Trim()) -eq "$SA/@2/%2") {
    Write-Pass "stdout alone carries the value, never an 'ERROR:' line"
} else {
    Write-Fail "stdout alone was '$((($only | Out-String)).Trim())' rc=$onlyRc"
}

Write-Section "ROW 3: -t %9999 is 'can't find pane', exit 1, on stderr"
$r = Disp '%9999'
if ($r.rc -eq 1 -and $r.all -match "can't find pane: %9999" -and $r.all -notmatch 'ERROR:') {
    Write-Pass "-t %9999 -> $($r.all) rc=1"
} else {
    Write-Fail "-t %9999 -> '$($r.all)' rc=$($r.rc), expected exit 1 + can't find pane: %9999"
}

Write-Section "ROW 4: -t @9999 is 'can't find window', exit 1, on stderr"
$r = Disp '@9999'
$split = Split-Streams @('-L', $NS, '-f', $conf, 'display-message', '-p', '-t', '@9999', $FMT)
$soOnly = $split.stdout
$se     = $split.stderr
if ($r.rc -eq 1 -and $r.all -match "can't find window: @9999") {
    Write-Pass "-t @9999 -> $($r.all) rc=1"
} else {
    Write-Fail "-t @9999 -> '$($r.all)' rc=$($r.rc), expected exit 1 + can't find window: @9999"
}
if ($r.all -notmatch 'ERROR:' -and $r.all -match 'psmux:') {
    Write-Pass "diagnostic uses the client's 'psmux: ' prefix, not 'ERROR:'"
} else {
    Write-Fail "diagnostic prefix is wrong: '$($r.all)'"
}
if ([string]::IsNullOrWhiteSpace($soOnly)) {
    Write-Pass "stdout is EMPTY on the failure (a script reading it gets nothing, not a fake value)"
} else {
    Write-Fail "stdout carried '$($soOnly.Trim())' on a failing target"
}
if ($se -match "can't find window: @9999" -and $split.rc -eq 1) {
    Write-Pass "the diagnostic really is on stderr, with exit 1 ($($se.Trim()))"
} else {
    Write-Fail "stderr='$($se.Trim())' rc=$($split.rc), expected the diagnostic on stderr at exit 1"
}

Write-Section "ROW 5: qualified forms keep working"
foreach ($pair in @(@("${SA}:@2", "$SA/@2/%2"), @("${SA}:@2.%3", "$SA/@2/%3"), @("${SB}:@1", "$SB/@1/%1"))) {
    $r = Disp $pair[0]
    if ($r.rc -eq 0 -and $r.stdout -eq $pair[1]) {
        Write-Pass "-t $($pair[0]) -> $($r.stdout)"
    } else {
        Write-Fail "-t $($pair[0]) -> '$($r.stdout)' rc=$($r.rc), expected '$($pair[1])'"
    }
}

# ── unqualified @N with a pane component ───────────────────────────────────
Write-Section "UNQUALIFIED @N.<pane>: the pane component resolves inside the owner"
foreach ($pair in @(@('@2.%3', "$SA/@2/%3"), @('@2.0', "$SA/@2/%2"))) {
    $r = Disp $pair[0]
    if ($r.rc -eq 0 -and $r.stdout -eq $pair[1]) {
        Write-Pass "-t $($pair[0]) -> $($r.stdout)"
    } else {
        Write-Fail "-t $($pair[0]) -> '$($r.stdout)' rc=$($r.rc), expected '$($pair[1])'"
    }
}

# ── the resolution must be ROUTING, not a lucky format expansion ───────────
Write-Section "SIDE EFFECTS: send-keys -t %N types into the OWNING pane"
# %3 lives only in alpha's second window. Type a marker and read it back from
# that exact pane; beta's pane must stay clean.
$marker = 'I627MARK' + [guid]::NewGuid().ToString('N').Substring(0,6)
Px @('send-keys', '-t', '%3', "echo $marker", 'Enter') | Out-Null
Start-Sleep -Milliseconds 1500
$capOwner = Px @('capture-pane', '-p', '-t', "${SA}:@2.%3")
$capOther = Px @('capture-pane', '-p', '-t', "${SB}:@1.%1")
if ($capOwner.stdout -match [regex]::Escape($marker)) {
    Write-Pass "the marker landed in ${SA}'s %3"
} else {
    Write-Fail "the marker is NOT in ${SA}'s %3; capture was:`n$($capOwner.stdout)"
}
if ($capOther.stdout -notmatch [regex]::Escape($marker)) {
    Write-Pass "$SB's pane was untouched (no recency misfire)"
} else {
    Write-Fail "the marker leaked into $SB's pane"
}

Write-Section "SIDE EFFECTS: capture-pane -t %N reads the OWNING pane"
$cap = Px @('capture-pane', '-p', '-t', '%3')
if ($cap.rc -eq 0 -and $cap.stdout -match [regex]::Escape($marker)) {
    Write-Pass "capture-pane -t %3 returned ${SA}'s pane content"
} else {
    Write-Fail "capture-pane -t %3 rc=$($cap.rc) did not return the owning pane:`n$($cap.stdout)"
}

# ── ambiguity is still refused (issue #569) ────────────────────────────────
Write-Section "AMBIGUITY: several owners and the routed session owns none -> exit 1"
# gamma is created LAST, so recency routes there, and gamma has only one window
# with one pane (%1). %2 is then owned by alpha AND beta-after-a-split, which is
# exactly the shape #569 refuses.
Px @('split-window', '-d', '-t', "${SB}:@1") | Out-Null
Start-Sleep -Milliseconds 500
Px @('new-session', '-d', '-s', $SG, '-n', 'g1') | Out-Null
Start-Sleep -Milliseconds 700
$owners = Px @('list-panes', '-a', '-F', '#{session_name} #{pane_id}')
Write-Info "owners now:`n$($owners.stdout)"

# The recency fallback is also insufficient when it happens to hold the id
# along with other sessions (issue #1078). A current pane in that same session
# provides real context and may still use those ids.
foreach ($pair in @(@('%1', 'pane'), @('@1', 'window'))) {
    $id, $kind = $pair
    $r = Split-Streams @('-L', $NS, '-f', $conf, 'display-message', '-p', '-t', $id, $FMT)
    if ($r.rc -eq 1 -and -not $r.stdout.Trim() -and $r.stderr -match "ambiguous $kind id $id") {
        Write-Pass "bare $id refused without current pane context"
    } else {
        Write-Fail "bare $id -> stdout='$($r.stdout)' stderr='$($r.stderr)' rc=$($r.rc)"
    }
}

$port = (Get-Content -LiteralPath (Join-Path $root "${NS}__${SG}.port") -Raw).Trim()
$env:TMUX = "fixture,$port,0"
try {
    foreach ($id in @('%1', '@1')) {
        $r = Disp $id
        if ($r.rc -eq 0 -and $r.stdout -eq "$SG/@1/%1") {
            Write-Pass "bare $id addresses the current pane's session"
        } else {
            Write-Fail "bare $id with current pane context -> '$($r.all)' rc=$($r.rc)"
        }
    }
} finally {
    $env:TMUX = $null
}

$r = Disp '%2'
if ($r.rc -eq 1 -and $r.all -match 'ambiguous pane id %2' -and $r.all -match 'qualify as session:window.pane') {
    Write-Pass "-t %2 refused: $($r.all)"
} else {
    Write-Fail "-t %2 -> '$($r.all)' rc=$($r.rc), expected exit 1 + 'ambiguous pane id %2'"
}
if ($r.all -match [regex]::Escape($SA) -and $r.all -match [regex]::Escape($SB)) {
    Write-Pass "the refusal names both owning sessions by their SHORT (typeable) names"
} else {
    Write-Fail "the refusal did not name both owners: '$($r.all)'"
}
if ($r.all -notmatch [regex]::Escape("${NS}__")) {
    Write-Pass "the refusal does not leak the '<ns>__<session>' registry base name"
} else {
    Write-Fail "the refusal leaked a registry base name: '$($r.all)'"
}
# @2 is STILL unique to alpha even now, so it must keep resolving. Only the
# session and window are asserted here: which pane of @2 is active depends on
# the temp-focus history of the earlier send-keys, which is not what #627 is
# about.
$r = Disp '@2'
if ($r.rc -eq 0 -and $r.stdout -like "$SA/@2/*") {
    Write-Pass "-t @2 still resolves to its unique owner with three sessions up ($($r.stdout))"
} else {
    Write-Fail "-t @2 -> '$($r.stdout)' rc=$($r.rc), expected '$SA/@2/<pane>'"
}

# ── Win32 TUI check: a really attached client ──────────────────────────────
Write-Section "WIN32 TUI: an attached window addressed by its bare %N from outside"
$tuiSess = 'i627tui'
$tuiOk = $false
try {
    # A separate namespace so the attached client cannot be confused with the
    # rig above, and PSMUX_SESSION is scrubbed for the child by construction.
    $tuiNs = $NS + 'tui'
    $bat = Join-Path $rig 'launch_tui.cmd'
    @(
        '@echo off'
        'set "PSMUX_SESSION="'
        'set "PSMUX_SESSION_NAME="'
        'set "PSMUX_PANE="'
        'set "TMUX="'
        "set `"PSMUX_DATA_DIR=$root`""
        'set "PSMUX_NO_WARM=1"'
        "`"$PSMUX`" -L $tuiNs -f `"$conf`" new-session -s $tuiSess -n tuiw"
    ) | Set-Content -Path $bat -Encoding ascii
    $proc = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', "`"$bat`"" -PassThru
    Start-Sleep -Seconds 4

    $tuiPanes = Run @('-L', $tuiNs, '-f', $conf, 'list-panes', '-a', '-F', '#{session_name} #{window_id} #{pane_id}')
    Write-Info "attached rig: $($tuiPanes.stdout -replace "`n", ' | ')"
    if ($tuiPanes.stdout -match '%1') {
        $tuiOk = $true
        $d = Run @('-L', $tuiNs, '-f', $conf, 'display-message', '-p', '-t', '%1', $FMT)
        if ($d.rc -eq 0 -and $d.stdout -eq "$tuiSess/@1/%1") {
            Write-Pass "bare -t %1 resolved against the live attached session: $($d.stdout)"
        } else {
            Write-Fail "bare -t %1 on the attached session -> '$($d.stdout)' rc=$($d.rc)"
        }
        $tuiMark = 'I627TUI' + [guid]::NewGuid().ToString('N').Substring(0,6)
        Run @('-L', $tuiNs, '-f', $conf, 'send-keys', '-t', '%1', "echo $tuiMark", 'Enter') | Out-Null
        Start-Sleep -Milliseconds 1800
        $c = Run @('-L', $tuiNs, '-f', $conf, 'capture-pane', '-p', '-t', '%1')
        if ($c.stdout -match [regex]::Escape($tuiMark)) {
            Write-Pass "send-keys -t %1 reached the real attached pane (marker read back)"
        } else {
            Write-Fail "the marker never appeared in the attached pane:`n$($c.stdout)"
        }
        $d = Run @('-L', $tuiNs, '-f', $conf, 'display-message', '-p', '-t', '@4242', $FMT)
        if ($d.rc -eq 1 -and $d.all -match "can't find window: @4242") {
            Write-Pass "a bad @N against a live attached session still exits 1"
        } else {
            Write-Fail "bad @N on the attached session -> '$($d.all)' rc=$($d.rc)"
        }
    } else {
        Write-Info "attached client did not come up; skipping the TUI assertions"
    }
} finally {
    if ($tuiOk) { Run @('-L', ($NS + 'tui'), 'kill-session', '-t', $tuiSess) | Out-Null }
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
}

# ── teardown ───────────────────────────────────────────────────────────────
Write-Section "TEARDOWN"
foreach ($s in @($SA, $SB, $SG)) { Px @('kill-session', '-t', $s) | Out-Null }
Start-Sleep -Milliseconds 800
Remove-Item -Recurse -Force $rig -EA SilentlyContinue

Write-Host ""
Write-Host "PASS: $script:Pass  FAIL: $script:Fail" -ForegroundColor $(if ($script:Fail -eq 0) { 'Green' } else { 'Red' })
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
