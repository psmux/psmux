# Issue #647 (WIN-03): control characters in formatted output.
#
# The reporter drives psmux from a gateway controller that reads one record per
# pane out of `list-panes -F`, and asked for psmux to match what native tmux
# does with a control byte in a format result.
#
# Ground truth was measured against tmux 3.4 under WSL with LANG=C.UTF-8 and
# cross checked against the tmux sources. tmux has two separate mechanisms:
#
#   1. `display-message -p` prints through server_client_print(tc, 0, evb)
#      (cmd-display-message.c:152) and the parse == 0 arm always encodes the
#      result with VIS_OCTAL|VIS_CSTYLE|VIS_NOSLASH (server-client.c:3089-3091):
#        "A<TAB>B" => 41 09 42        tab kept, VIS_TAB is not set
#        "A<CR>B"  => 41 5C 72 42     `A\rB`
#        "A<ESC>B" => `A\033B`
#        "A<BEL>B" => `A\aB`
#        "a<DEL>b" => `a\177b`
#        "A<U+2713>B" => 41 E2 9C 93 42   valid UTF-8 kept
#
#   2. Window and session names go through clean_name (tmux.c:303-317) with
#      VIS_OCTAL|VIS_CSTYLE|VIS_TAB|VIS_NL at the moment they are set, so a
#      name can never hold a raw control byte:
#        rename-window "w<TAB>x" then #{window_name} => `w\tx`
#        rename-window "w<ESC>y" then #{window_name} => `w\033y`
#
# What tmux does NOT do, and psmux must not do either: sanitize the `list-*`
# output itself. tmux 3.6 removed that (commit 5fd45b38 "Do not strvis output
# to terminal from commands."), so a literal tab the caller put in their own
# `-F` string still reaches stdout byte for byte. capture-pane, show-buffer and
# save-buffer are likewise untouched.
# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$ErrorActionPreference = "Continue"
if ($env:PSMUX_TEST_EXE) { $PSMUX = $env:PSMUX_TEST_EXE }
else { $PSMUX = (Get-Command psmux -EA Stop).Source }
$SESSION = "t647fmt"
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Hex($s) {
    if ($null -eq $s) { return '<null>' }
    return [BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($s))
}
function Assert-Hex($label, $actual, $expected) {
    $h = Hex $actual
    if ($h -eq $expected) { Write-Pass "$label => $h" }
    else { Write-Fail "$label => $h, expected $expected" }
}

$TAB = [char]9; $CR = [char]13; $LF = [char]10; $ESC = [char]27; $BEL = [char]7; $NUL = [char]0
$CHECK = [char]0x2713

$env:PSMUX_NO_WARM = "1"
function Cleanup { & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null }

Cleanup
Start-Sleep -Milliseconds 500
& $PSMUX new-session -d -s $SESSION -n zero 2>&1 | Out-Null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "session creation failed"; exit 1 }
$pane = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -First 1).Trim()

Write-Host "`n=== Issue #647 WIN-03: control characters in -F output ===" -ForegroundColor Cyan

# --- Arm 1: display-message -p, literal control bytes in the format text ---
Write-Host "[Arm 1] display-message -p encodes control bytes (tmux VIS_OCTAL|VIS_CSTYLE|VIS_NOSLASH)" -ForegroundColor Yellow
Assert-Hex "literal TAB   (kept)"  ((& $PSMUX display-message -t $pane -p "A${TAB}B") -join '') '41-09-42'
Assert-Hex "literal CR    (\r)"    ((& $PSMUX display-message -t $pane -p "A${CR}B")  -join '') '41-5C-72-42'
Assert-Hex "literal ESC   (\033)"  ((& $PSMUX display-message -t $pane -p "A${ESC}B") -join '') '41-5C-30-33-33-42'
Assert-Hex "literal BEL   (\a)"    ((& $PSMUX display-message -t $pane -p "A${BEL}B") -join '') '41-5C-61-42'
Assert-Hex "literal UTF-8 (kept)"  ((& $PSMUX display-message -t $pane -p "A${CHECK}B") -join '') '41-E2-9C-93-42'
Assert-Hex "plain ASCII   (kept)"  ((& $PSMUX display-message -t $pane -p "A B") -join '') '41-20-42'
# VIS_NOSLASH: a Windows path in a message keeps single separators.
Assert-Hex "backslash     (kept)"  ((& $PSMUX display-message -t $pane -p 'C:\src') -join '') '43-3A-5C-73-72-63'

# --- Arm 2: control bytes carried in a VARIABLE value ---
Write-Host "[Arm 2] control bytes inside a format variable" -ForegroundColor Yellow
& $PSMUX set-option -g '@i647t' "a${TAB}b" 2>&1 | Out-Null
& $PSMUX set-option -g '@i647e' "a${ESC}b" 2>&1 | Out-Null
Assert-Hex "#{@i647t} through display-message" ((& $PSMUX display-message -t $pane -p '#{@i647t}') -join '') '61-09-62'
Assert-Hex "#{@i647e} through display-message" ((& $PSMUX display-message -t $pane -p '#{@i647e}') -join '') '61-5C-30-33-33-62'

# --- Arm 3: a window renamed to contain a control byte ---
Write-Host "[Arm 3] a window name can never hold a raw control byte (tmux clean_name)" -ForegroundColor Yellow
& $PSMUX rename-window -t "${SESSION}:0" "w${TAB}x" 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$wn = ((& $PSMUX list-windows -t $SESSION -F '#{window_name}') | Select-Object -First 1)
Assert-Hex "rename-window w<TAB>x => w\tx" $wn '77-5C-74-78'

& $PSMUX rename-window -t "${SESSION}:0" "w${ESC}y" 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$wn = ((& $PSMUX list-windows -t $SESSION -F '#{window_name}') | Select-Object -First 1)
Assert-Hex "rename-window w<ESC>y => w\033y" $wn '77-5C-30-33-33-79'

& $PSMUX rename-window -t "${SESSION}:0" "w${CR}z" 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$wn = ((& $PSMUX list-windows -t $SESSION -F '#{window_name}') | Select-Object -First 1)
Assert-Hex "rename-window w<CR>z => w\rz" $wn '77-5C-72-7A'

# A NUL cannot reach psmux through argv at all: the Win32 command line is NUL
# terminated, so the OS truncates the argument before psmux sees it (execve on
# Linux does the same). What matters is that no NUL ends up in the stored name;
# the backslash-zero encoding itself is covered by the Rust unit tests.
& $PSMUX rename-window -t "${SESSION}:0" "w${NUL}n" 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$wn = ((& $PSMUX list-windows -t $SESSION -F '#{window_name}') | Select-Object -First 1)
if ((Hex $wn) -notmatch '(^|-)00(-|$)') { Write-Pass "a NUL never reaches the stored window name ($(Hex $wn))" }
else { Write-Fail "the stored window name carries a NUL ($(Hex $wn))" }

# A path shaped name keeps single backslashes: the one deliberate deviation
# from tmux, which doubles them.
& $PSMUX rename-window -t "${SESSION}:0" 'C:\src' 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$wn = ((& $PSMUX list-windows -t $SESSION -F '#{window_name}') | Select-Object -First 1)
Assert-Hex "rename-window C:\src keeps one backslash" $wn '43-3A-5C-73-72-63'

# --- Arm 4: the reporter's record is now a safe single line ---
Write-Host "[Arm 4] the reporter's tab separated record" -ForegroundColor Yellow
& $PSMUX rename-window -t "${SESSION}:0" "zero${TAB}injected" 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$line = ((& $PSMUX list-panes -t $SESSION -F "#{pane_id}${TAB}#{window_name}${TAB}#{pane_current_command}") | Select-Object -First 1)
$tabs = ([regex]::Matches($line, "`t")).Count
if ($tabs -eq 2) { Write-Pass "record has exactly 2 separators despite a tab in the window name (bytes: $(Hex $line))" }
else { Write-Fail "record has $tabs separators, expected 2 (bytes: $(Hex $line))" }
if ($line -notmatch "`r" -and $line -notmatch [char]27) { Write-Pass "record carries no CR and no ESC" }
else { Write-Fail "record still carries a raw control byte" }
# The caller's own separators survive: tmux 3.6+ does not touch -F output.
if ($line -match '^%\d+\t') { Write-Pass "list-panes -F keeps the caller's literal TAB separator (tmux 3.6+ parity)" }
else { Write-Fail "list-panes -F lost the caller's literal TAB separator" }
& $PSMUX rename-window -t "${SESSION}:0" zero 2>&1 | Out-Null

# --- Arm 5: what must NOT be sanitized ---
Write-Host "[Arm 5] capture-pane, show-buffer and save-buffer stay byte exact" -ForegroundColor Yellow
& $PSMUX set-buffer "p${TAB}q" 2>&1 | Out-Null
Assert-Hex "show-buffer keeps a raw TAB" ((& $PSMUX show-buffer) -join '') '70-09-71'
$bufFile = Join-Path $env:TEMP "i647buf.txt"
Remove-Item $bufFile -Force -EA SilentlyContinue
& $PSMUX save-buffer $bufFile 2>&1 | Out-Null
if (Test-Path $bufFile) {
    $bytes = [IO.File]::ReadAllBytes($bufFile)
    if ($bytes -contains 9) { Write-Pass "save-buffer wrote a raw TAB ($([BitConverter]::ToString($bytes)))" }
    else { Write-Fail "save-buffer lost the raw TAB ($([BitConverter]::ToString($bytes)))" }
    Remove-Item $bufFile -Force -EA SilentlyContinue
} else {
    Write-Fail "save-buffer wrote no file"
}

# capture-pane must hand back the pane's own bytes. A tab is not a useful
# probe: the terminal grid has no tab cell, so both tmux and psmux return the
# expanded spaces. Probe with a multi byte character instead. Under tmux's non
# UTF-8 client rule that character would be replaced with an underscore
# (utf8.c:783-810 utf8_sanitize), and capture-pane is on the list of paths that
# must never be touched, so it has to come back as raw UTF-8.
& $PSMUX send-keys -t $pane "[Console]::Out.Write(`"X`" + [char]0x2713 + `"Y`" + [char]10)" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4
$cap = (& $PSMUX capture-pane -p -t $pane) -join "`n"
$xy = ($cap -split "`n" | Where-Object { $_ -match '^X' } | Select-Object -First 1)
if ($xy) {
    $xyHex = Hex $xy
    if ($xyHex -match 'E2-') { Write-Pass "capture-pane returns raw multi byte UTF-8, unsanitized ($xyHex)" }
    else { Write-Fail "capture-pane mangled the multi byte character ($xyHex)" }
    if ($xy -notmatch '\\033' -and $xy -notmatch '\\0') { Write-Pass "capture-pane applies no visual escaping" }
    else { Write-Fail "capture-pane output carries a visual escape ($xyHex)" }
} else {
    Write-Host "  [SKIP] pane did not echo the probe line; capture-pane check inconclusive" -ForegroundColor DarkYellow
}

Cleanup
Start-Sleep -Milliseconds 500

# ---------------------------------------------------------------------------
# Win32 TUI verification: the same contract on the attached command route,
# which dispatches display-message and rename-window through a second handler.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host ("=" * 60)
Write-Host "Win32 TUI VISUAL VERIFICATION"
Write-Host ("=" * 60)

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION,"-n","zero" -PassThru
Start-Sleep -Seconds 6
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "TUI: attached session did not start"
} else {
    $tpane = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -First 1).Trim()
    Assert-Hex "TUI: display-message -p ESC" ((& $PSMUX display-message -t $tpane -p "A${ESC}B") -join '') '41-5C-30-33-33-42'
    Assert-Hex "TUI: display-message -p TAB kept" ((& $PSMUX display-message -t $tpane -p "A${TAB}B") -join '') '41-09-42'

    & $PSMUX rename-window -t "${SESSION}:0" "t${TAB}u" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $wn = ((& $PSMUX list-windows -t $SESSION -F '#{window_name}') | Select-Object -First 1)
    Assert-Hex "TUI: rename-window t<TAB>u => t\tu" $wn '74-5C-74-75'

    $line = ((& $PSMUX list-panes -t $SESSION -F "#{pane_id}${TAB}#{window_name}") | Select-Object -First 1)
    $tabs = ([regex]::Matches($line, "`t")).Count
    if ($tabs -eq 1) { Write-Pass "TUI: attached record keeps exactly one separator ($(Hex $line))" }
    else { Write-Fail "TUI: attached record has $tabs separators ($(Hex $line))" }
}

Cleanup
if ($proc) { try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {} }
Start-Sleep -Milliseconds 500

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
