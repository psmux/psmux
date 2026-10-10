# The tree and session choosers killed on `x` with nothing asked. tmux puts a
# question up first and kills only on `y`: `window-tree.c:1282-1310` builds
# "Kill session %s? ", "Kill window %u? " or "Kill pane %u? " and hands it to
# `status_prompt_set(... PROMPT_SINGLE ...)`, and the callback at `:1137`
# returns without killing unless the key was `y`. psmux already asks for the
# same act from a key binding (`prefix &` and `prefix x`), so the chooser was
# the one place a single keystroke destroyed a window or a session.
#
# PART 1  Source proof: `x` only puts the question up, the kill hangs off the
#         answer, and the answer checks the row the question named.
# PART 2  Live proof: a real attached client, real keys through
#         WriteConsoleInput, and the window and session counts read back over
#         the wire.
#
# The suite runs in a PSMUX_DATA_DIR of its own, so the only sessions the
# choosers can see are the ones it made: a `y` cannot reach a developer's own
# session even if the cursor were somewhere unexpected.
#
# One thing this suite deliberately does not assert, because it is the same
# before this change: a kill that names the session the client is attached to
# detaches the client and leaves the session alive, where tmux kills it and the
# client then detaches (measured on tmux 3.7c, `server_destroy_session` in
# `window_tree_kill_each`, window-tree.c:1113-1114). That difference is older
# than this change and belongs to its own issue.

$ErrorActionPreference = "Continue"
$script:pass = 0
$script:fail = 0
$script:results = @()

function Write-Test($msg) { Write-Host "  TEST: $msg" -ForegroundColor Yellow }
function Write-Pass($msg) { Write-Host "  PASS: $msg" -ForegroundColor Green; $script:pass++ }
function Write-Fail($msg) { Write-Host "  FAIL: $msg" -ForegroundColor Red; $script:fail++ }
function Add-Result($name, $ok, $detail) {
    if ($ok) { Write-Pass "$name $detail" } else { Write-Fail "$name $detail" }
    $script:results += [PSCustomObject]@{ Test = $name; Pass = $ok; Detail = $detail }
}

# A config of this suite's own, so the prefix is the default C-b and no
# personal `set -g` reaches the client (discussion #748).
. "$PSScriptRoot\isolated_config.ps1"

# A client started from inside a session refuses to attach ("sessions should be
# nested with care"), so the suite's own clients need this cleared.
$env:PSMUX_SESSION = ""

# A registry of this suite's own: the choosers list what they find here.
$script:dataDir = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_kcq_" + [guid]::NewGuid().ToString('N').Substring(0, 10))
New-Item -ItemType Directory -Force $script:dataDir | Out-Null
$env:PSMUX_DATA_DIR = $script:dataDir

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path
if (-not $PSMUX) {
    $cmd = Get-Command psmux -EA SilentlyContinue
    if ($cmd) { $PSMUX = $cmd.Source }
}
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

Write-Host "`n=== Chooser kill confirmation ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $script:dataDir"

# ════════════════════════════════════════════════════════════════════
# PART 1  Source proof
# ════════════════════════════════════════════════════════════════════

$src = Get-Content (Join-Path $PSScriptRoot "..\src\client.rs") -Raw

Write-Test "tree chooser: 'x' sets the question instead of killing"
$treeAsks = $src -match "KeyCode::Char\('x'\)\s+if\s+tree_chooser\s*=>\s*\{(?:(?!=>)[\s\S])*?chooser_kill = Some\(ChooserKill \{(?:(?!\}\))[\s\S])*?kill-window"
Add-Result "tree x sets chooser_kill" $treeAsks ""

Write-Test "tree chooser: 'x' no longer pushes the kill itself"
$treeArm = [regex]::Match($src, "KeyCode::Char\('x'\)\s+if\s+tree_chooser\s*=>\s*\{(?:(?!KeyCode::)[\s\S])*")
$treeArmText = if ($treeArm.Success) { $treeArm.Value } else { "" }
$treeNoKill = $treeArmText -and ($treeArmText -notmatch "kill-window\\n")
Add-Result "tree x pushes no kill-window" $treeNoKill ""

Write-Test "session chooser: 'x' sets the question instead of killing"
$sessAsks = $src -match "KeyCode::Char\('x'\)\s+if\s+session_chooser\s*=>\s*\{(?:(?!=>)[\s\S])*?chooser_kill = Some\(ChooserKill \{(?:(?!\}\))[\s\S])*?kill-session"
Add-Result "session x sets chooser_kill" $sessAsks ""

Write-Test "the kill hangs off the answer, and only a yes runs it"
$answerArm = $src -match "code if chooser_kill\.is_some\(\)\s*=>\s*\{(?:(?!=>)[\s\S])*?chooser_kill\.take\(\)\.filter\(\|_\| chooser_kill_confirmed\(code\)\)"
Add-Result "answer arm guarded by chooser_kill_confirmed" $answerArm ""

Write-Test "only y and Y count as yes, as tmux kills only on 'y'"
$yesRule = $src -match "fn chooser_kill_confirmed\(code: KeyCode\) -> bool \{\s*matches!\(code, KeyCode::Char\('y'\) \| KeyCode::Char\('Y'\)\)"
Add-Result "chooser_kill_confirmed takes y and Y alone" $yesRule ""

Write-Test "the answer acts only on the row the question named"
$treeRowChecked = $src -match "&& Some\(wid\) == asked\.window\s*\r?\n\s*&& sess_name == asked\.session"
$sessRowChecked = $src -match "\.filter\(\|sname\| \*sname == asked\.session\)"
Add-Result "tree answer checks the window it asked about" $treeRowChecked ""
Add-Result "session answer checks the session it asked about" $sessRowChecked ""

Write-Test "Escape answers no and leaves the chooser open"
$escCancels = $src -match "if chooser_kill\.is_some\(\) \{(?:(?!\} else)[\s\S])*?chooser_kill = None;"
Add-Result "Escape clears only the question" $escCancels ""

Write-Test "the question is drawn in the confirm box"
$drawn = $src -match "\.or\(chooser_kill\.as_ref\(\)\.map\(\|k\| k\.prompt\.as_str\(\)\)\)"
Add-Result "confirm box draws the question" $drawn ""

Write-Test "a jump key cannot move the cursor while the question is up"
$jumpGated = $src -match "if tree_chooser && chooser_kill\.is_none\(\) \{"
Add-Result "digit jump gated on no pending question" $jumpGated ""

Write-Test "docs/keybindings.md says the chooser kill asks"
$docs = Get-Content (Join-Path $PSScriptRoot "..\docs\keybindings.md") -Raw
$docsSay = $docs -match "Kill the highlighted entry[^|]*with confirmation"
Add-Result "docs record the confirmation" $docsSay ""

# ════════════════════════════════════════════════════════════════════
# PART 2  Live proof
# ════════════════════════════════════════════════════════════════════

$injectorExe = "$env:TEMP\psmux_injector.exe"
$injectorSrc = Join-Path $PSScriptRoot "injector.cs"
if (-not (Test-Path $injectorExe) -or ((Get-Item $injectorSrc).LastWriteTime -gt (Get-Item $injectorExe).LastWriteTime)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) {
        $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
    }
    & $csc /nologo /optimize /out:$injectorExe $injectorSrc 2>&1 | Out-Null
}
$haveInjector = Test-Path $injectorExe
Add-Result "injector compiled" $haveInjector $injectorExe

$TREE = "kcq_tree"
$SA   = "kcq_a"
$SB   = "kcq_b"

function Window-Count($session) {
    $out = & $PSMUX list-windows -t $session 2>$null
    if (-not $out) { return 0 }
    return ([string[]]$out).Count
}
function Session-Alive($name) {
    & $PSMUX has-session -t $name 2>$null | Out-Null
    return ($LASTEXITCODE -eq 0)
}
function Attach-Client($session) {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "attach", "-t", $session -PassThru
    Start-Sleep -Seconds 4
    return $p
}
function Stop-Client($p) {
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 800
}

if ($haveInjector) {
    # ---- tree chooser: three windows, and only `y` loses one ----
    & $PSMUX new-session -d -s $TREE 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    & $PSMUX new-window -t $TREE 2>&1 | Out-Null
    & $PSMUX new-window -t $TREE 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    $before = Window-Count $TREE
    Add-Result "tree: three windows to start with" ($before -eq 3) "count=$before"

    $client = Attach-Client $TREE

    Write-Test "Live: prefix w, then x alone"
    & $injectorExe $client.Id "^b{SLEEP:500}w{SLEEP:900}x{SLEEP:900}" | Out-Null
    Start-Sleep -Milliseconds 800
    $afterAsk = Window-Count $TREE
    Add-Result "tree: x alone kills nothing" ($afterAsk -eq $before) "count=$afterAsk"

    Write-Test "Live: n cancels the question"
    & $injectorExe $client.Id "n{SLEEP:900}" | Out-Null
    Start-Sleep -Milliseconds 700
    $afterNo = Window-Count $TREE
    Add-Result "tree: n leaves the window alive" ($afterNo -eq $before) "count=$afterNo"

    Write-Test "Live: x then y kills one window"
    & $injectorExe $client.Id "x{SLEEP:800}y{SLEEP:1200}" | Out-Null
    Start-Sleep -Seconds 1
    $afterYes = Window-Count $TREE
    Add-Result "tree: y kills exactly one window" ($afterYes -eq ($before - 1)) "count=$afterYes"

    Write-Test "Live: the chooser is still usable, so x then y kills a second one"
    & $injectorExe $client.Id "x{SLEEP:1000}y{SLEEP:1500}" | Out-Null
    Start-Sleep -Seconds 2
    $afterTwo = Window-Count $TREE
    Add-Result "tree: a second question and yes kills one more" ($afterTwo -eq ($before - 2)) "count=$afterTwo"

    Stop-Client $client
    & $PSMUX kill-session -t $TREE 2>$null | Out-Null

    # ---- session chooser: ask, cancel with Escape, then kill on y ----
    # The chooser opens on the current session's row, so one `j` is the other
    # session, and these are the only two rows the registry holds.
    & $PSMUX new-session -d -s $SA 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    & $PSMUX new-session -d -s $SB 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Add-Result "session: both test sessions started" ((Session-Alive $SA) -and (Session-Alive $SB)) ""

    $client = Attach-Client $SA

    Write-Test "Live: prefix s, then x alone on the current session's row"
    & $injectorExe $client.Id "^b{SLEEP:500}s{SLEEP:900}x{SLEEP:900}" | Out-Null
    Start-Sleep -Milliseconds 800
    Add-Result "session: x alone kills nothing" ((Session-Alive $SA) -and (Session-Alive $SB)) ""

    Write-Test "Live: Escape answers no and the chooser stays open"
    & $injectorExe $client.Id "{ESC}{SLEEP:900}" | Out-Null
    Start-Sleep -Milliseconds 700
    Add-Result "session: Escape kills nothing" ((Session-Alive $SA) -and (Session-Alive $SB)) ""

    Write-Test "Live: j moves to the other session, x asks about it"
    & $injectorExe $client.Id "j{SLEEP:500}x{SLEEP:900}" | Out-Null
    Start-Sleep -Milliseconds 800
    Add-Result "session: x on the other row kills nothing" ((Session-Alive $SA) -and (Session-Alive $SB)) ""

    Write-Test "Live: y kills the session the question named"
    & $injectorExe $client.Id "y{SLEEP:1500}" | Out-Null
    Start-Sleep -Seconds 2
    $sbGone = -not (Session-Alive $SB)
    $saKept = Session-Alive $SA
    Add-Result "session: y killed the other session" $sbGone ""
    Add-Result "session: the attached session is untouched" $saKept ""

    Stop-Client $client
} else {
    Add-Result "live tests" $false "skipped (injector missing)"
}

# ════════════════════════════════════════════════════════════════════
# Cleanup
# ════════════════════════════════════════════════════════════════════
foreach ($s in @($TREE, $SA, $SB)) { & $PSMUX kill-session -t $s 2>$null | Out-Null }
Start-Sleep -Milliseconds 500
& $PSMUX kill-server 2>$null | Out-Null
Remove-PsmuxIsolatedConfig
$env:PSMUX_DATA_DIR = $null
Remove-Item -Recurse -Force $script:dataDir -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $pass / $($pass + $fail)" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Yellow' })
foreach ($r in $results) {
    $color  = if ($r.Pass) { 'Green' } else { 'Red' }
    $status = if ($r.Pass) { 'PASS' } else { 'FAIL' }
    Write-Host "  [$status] $($r.Test) $($r.Detail)" -ForegroundColor $color
}

exit $fail
