# `codepoint-widths`: tmux's server option for overriding how many columns a
# Unicode codepoint occupies (options-table.c, parsed by
# utf8_add_to_width_cache in utf8.c, applied by utf8_width).
#
# psmux resolves East Asian AMBIGUOUS characters to width 1, exactly as tmux
# does. When the user's OUTER terminal draws them as two columns instead, the
# two disagree about where every following cell on the row starts and cells get
# stranded. That is the mechanism reproduced while investigating issue #639,
# whose reporter saw leftover glyphs after quitting `tig` -- whose commit graph
# is drawn with precisely those ambiguous width characters. This option lets an
# affected user tell psmux to agree with their terminal.
#
# WHAT THIS SCRIPT PROVES, AND WHY IT PROBES THE WAY IT DOES
#
# `show-options` echoing a value back proves nothing, so every assertion here
# is on RENDERED output from a live server.
#
# The probe is line WRAPPING. A pane $COLS columns wide holds $COLS ambiguous
# characters per row at width 1, but only $COLS/2 of them at width 2, so the
# row the text wraps on is a direct readout of the width psmux assigned. That
# decision is made by psmux's own emulator, which is what we are testing.
#
# Deliberately NOT probed here: overwriting one half of a wide glyph and
# checking the other half went with it. The pane's child runs under a ConPTY,
# and conhost has its OWN width opinion (it draws U+2502 as one column
# regardless of this option), so it repaints the line by its own geometry
# before psmux ever sees the bytes. Such a test would measure conhost, not
# psmux. That contract is asserted at the grid level instead, with no ConPTY in
# the way, by tests-rs/test_codepoint_widths.rs -- see
# overwriting_the_lead_half_clears_the_continuation and its siblings there.
# This is the same split tests/test_issue639_wide_char_clear.ps1 documents.

$ErrorActionPreference = "Continue"

# MANDATORY. Without this the console decodes capture-pane output in the OEM
# code page (CP437 on a stock box), every non-ASCII character arrives as "?",
# and the assertions below would compare mangled text against mangled text and
# report a meaningless pass. The CJK round trip gate near the top is the
# backstop if this is ever dropped.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$SOCK = "cpw"
# Separate namespace for the psmux.conf case, which needs a server started
# fresh with a config file rather than the anchored one the other cases share.
$SOCK2 = "cpwconf"
$COLS = 20
$ROWS = 8
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$work = Join-Path $env:TEMP "psmux_cpw"
New-Item -ItemType Directory -Force -Path $work | Out-Null

# This script creates and kills a session per case in quick succession. With
# the warm pane pool on, a new-session issued right after a kill can claim a
# spare shell the pool has not finished replacing, and the pane comes up empty:
# the failures alternate case by case in step with the pool's refill cycle,
# which looks exactly like a width bug and is not one. The widths under test
# are decided by the emulator and are unaffected by where the shell came from.
$env:PSMUX_NO_WARM = "1"

function Kill-Sess([string]$n, [string]$sock = $SOCK) {
    & $PSMUX -L $sock kill-session -t $n 2>&1 | Out-Null
}

# --- the payload -------------------------------------------------------------
# Emitted from a script file rather than through send-keys, because send-keys
# collapses interior whitespace and would confound the column assertions.
# Written with [Console]::Out.Write rather than Write-Host because NO_COLOR,
# which some shells export, strips escape sequences out of Write-Host.
$payload = Join-Path $work "cpw_payload.ps1"
@'
param([string]$Case, [int]$Count = 15)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$e = [char]27
function W([string]$s) { [Console]::Out.Write($s); [Console]::Out.Flush() }
$bar = [string][char]0x2502   # U+2502, East Asian AMBIGUOUS
$han = [string][char]0x4E2D   # U+4E2D, unambiguously double width
W "$e[H$e[2J"
switch ($Case) {
  "bars"  { W ($bar * $Count) }
  "han"   { W ($han * $Count) }
  "erase" { W ($bar * $Count); Start-Sleep -Milliseconds 400; W "$e[H$e[2J" }
  "cjk"   { W ($han * 3) }
}
Start-Sleep -Seconds 600
'@ | Set-Content -Path $payload -Encoding UTF8

# Run $Case in a fresh session and return the pane's non-empty rows.
# $Setup runs against the live session BEFORE the payload draws anything, which
# is the order that matters: like tmux, a width override applies to text drawn
# after it is set, not retroactively.
#
# $Sock defaults to the shared socket, which an anchor session keeps alive for
# the whole run. Without that anchor the server exits as soon as the previous
# case's session is killed (exit-empty is on by default), and the next case
# races a cold server start: the session is not there yet when capture-pane
# runs and the assertion sees an empty pane rather than a wrong width.
#
# $Expect is the codepoint the payload is supposed to draw. Under rapid session
# churn a pane occasionally comes up without ever running the payload, and the
# capture is then empty for harness reasons that have nothing to do with
# character widths. An empty capture is NOT evidence about a width, so the case
# is retried rather than scored; a capture that contains the glyph is scored on
# the first try, whatever shape it has, so a genuinely wrong width still fails.
# Pass -Expect 0 for cases (the erase case) where drawing nothing IS the answer.
function Get-Rows {
    param(
        [string]$Name,
        [string]$Case = "bars",
        [int]$Count = 15,
        [scriptblock]$Setup = $null,
        [string[]]$ServerArgs = @(),
        [string]$Sock = $SOCK,
        [int]$Expect = 0x2502,
        [int]$Tries = 4
    )
    for ($try = 1; $try -le $Tries; $try++) {
        Kill-Sess $Name $Sock
        # Let the previous case's teardown finish before asking for a new pane.
        Start-Sleep -Milliseconds 700
        $newArgs = @("-L", $Sock) + $ServerArgs + @("new-session", "-d", "-s", $Name, "-x", $COLS, "-y", $ROWS)
        & $PSMUX @newArgs 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        if ($Setup) { & $Setup }
        & $PSMUX -L $Sock send-keys -t $Name "pwsh -NoProfile -NoLogo -File '$payload' -Case $Case -Count $Count" Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 3000
        $cap = (& $PSMUX -L $Sock capture-pane -p -t $Name 2>&1 | Out-String)
        Kill-Sess $Name $Sock
        $rows = @($cap -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 })
        if ($Expect -eq 0) { return $rows }
        $found = 0
        foreach ($r in $rows) {
            $found += @($r.ToCharArray() | Where-Object { [int]$_ -eq $Expect }).Count
        }
        if ($found -gt 0) { return $rows }
        if ($try -lt $Tries) {
            Write-Host "    (retry $try/$Tries for $Name : the payload never drew)" -ForegroundColor DarkGray
        }
    }
    return @()
}

# Count occurrences of U+2502 in a row.
function Count-Bars([string]$s) {
    if (-not $s) { return 0 }
    return @($s.ToCharArray() | Where-Object { [int]$_ -eq 0x2502 }).Count
}

# Rows that carry any bar at all, as a "15" / "10,5" style shape signature.
function Bar-Shape([string[]]$rows) {
    $counts = @()
    foreach ($r in $rows) { $n = Count-Bars $r; if ($n -gt 0) { $counts += $n } }
    return ($counts -join ",")
}

# === ANCHOR ==================================================================
# Hold one idle session open for the whole run so the server never exits
# between cases. exit-empty is on by default, so without this the server tears
# itself down every time a case's session is killed and the next case races a
# cold start, producing empty captures that look like width failures.
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX -L $SOCK new-session -d -s cpw_anchor -x $COLS -y $ROWS 2>&1 | Out-Null
Start-Sleep -Milliseconds 1200

# === GATE ====================================================================
# Prove the harness can carry non-ASCII through capture-pane at all. Without
# this a CP437 console turns every glyph into "?" and every assertion below
# would compare mangled text against mangled text and pass for the wrong
# reason.
Write-Test "harness gate: non-ASCII survives a capture-pane round trip"
$gateRows = Get-Rows -Name "cpw_gate" -Case "cjk" -Expect 0x4E2D
$gateHan = 0
foreach ($r in $gateRows) {
    $gateHan += @($r.ToCharArray() | Where-Object { [int]$_ -eq 0x4E2D }).Count
}
if ($gateHan -lt 3) {
    Write-Fail "harness lost CJK on a round trip (got $gateHan of 3), results would be meaningless"
    Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    exit 1
}
Write-Pass "harness round trips CJK ($gateHan glyphs came back)"

# === BASELINE ================================================================
# The guard rail for this feature: with the option unset, nothing changes.
Write-Test "default: an ambiguous character still takes ONE column"
$rows = Get-Rows -Name "cpw_default" -Case "bars" -Count 15
$shape = Bar-Shape $rows
if ($shape -eq "15") {
    Write-Pass "15 ambiguous chars fit on one row of $COLS columns (shape $shape)"
} else {
    Write-Fail "expected all 15 on one row (shape '15'), got shape '$shape'"
}

# A genuinely wide character is the control: it already wraps at $COLS/2, with
# no option involved, which is what the override makes ambiguous chars do.
Write-Test "control: a genuinely wide character already takes TWO columns"
$rows = Get-Rows -Name "cpw_control" -Case "han" -Count 15 -Expect 0x4E2D
$hanCounts = @()
foreach ($r in $rows) {
    $n = @($r.ToCharArray() | Where-Object { [int]$_ -eq 0x4E2D }).Count
    if ($n -gt 0) { $hanCounts += $n }
}
$hanShape = ($hanCounts -join ",")
if ($hanShape -eq "10,5") {
    Write-Pass "15 wide chars wrap 10 + 5 on a $COLS column pane (shape $hanShape)"
} else {
    Write-Fail "expected wide chars to wrap '10,5', got '$hanShape'"
}

# === set -s ==================================================================
Write-Test "set -s codepoint-widths makes the ambiguous character TWO columns"
$rows = Get-Rows -Name "cpw_set" -Case "bars" -Count 15 -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2" 2>&1 | Out-Null
}
$shape = Bar-Shape $rows
if ($shape -eq "10,5") {
    Write-Pass "the override reserved two columns each: 15 chars wrapped 10 + 5 (shape $shape)"
} else {
    Write-Fail "expected the override to wrap '10,5', got '$shape'"
}

Write-Test "show-options -s reports the value"
& $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2" 2>&1 | Out-Null
$shown = (& $PSMUX -L $SOCK show-options -s codepoint-widths 2>&1 | Out-String).Trim()
# An array option: one `name[N] value` line per element, as tmux prints it.
if ($shown -eq "codepoint-widths[0] U+2502=2") {
    Write-Pass "show-options -s: $shown"
} else {
    Write-Fail "show-options -s did not report the value, got '$shown'"
}

# The array round trips through set / append / unset. Asserted here, next to
# the session that is guaranteed to be alive, rather than after a case's
# teardown where a restarted server would report an empty option and make the
# message lie about what happened.
& $PSMUX -L $SOCK set -sa codepoint-widths "U+4E2D=1" 2>&1 | Out-Null
$shown = (@(& $PSMUX -L $SOCK show-options -s -v codepoint-widths 2>&1) -join '|').Trim()
if ($shown -eq "U+2502=2|U+4E2D=1") {
    Write-Pass "set -sa appended an array ITEM rather than concatenating: '$shown'"
} else {
    Write-Fail "expected 'U+2502=2|U+4E2D=1' (one value a line) after append, got '$shown'"
}
& $PSMUX -L $SOCK set -su codepoint-widths 2>&1 | Out-Null
$shown = (& $PSMUX -L $SOCK show-options -s -v codepoint-widths 2>&1 | Out-String).Trim()
if ($shown -eq "") {
    Write-Pass "set -su cleared the array back to its empty default"
} else {
    Write-Fail "expected an empty value after -su, got '$shown'"
}

Write-Test "the range form applies to every codepoint in the range"
$rows = Get-Rows -Name "cpw_range" -Case "bars" -Count 15 -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2500-U+257F=2" 2>&1 | Out-Null
}
$shape = Bar-Shape $rows
if ($shape -eq "10,5") {
    Write-Pass "U+2502 picked up its width from the range U+2500-U+257F (shape $shape)"
} else {
    Write-Fail "expected the range to wrap '10,5', got '$shape'"
}

Write-Test "set -su restores the default width"
$rows = Get-Rows -Name "cpw_unset" -Case "bars" -Count 15 -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2" 2>&1 | Out-Null
    & $PSMUX -L $SOCK set -su codepoint-widths 2>&1 | Out-Null
}
$shape = Bar-Shape $rows
if ($shape -eq "15") {
    Write-Pass "after -su the character is one column again (shape $shape)"
} else {
    Write-Fail "expected '15' after unset, got '$shape'"
}

Write-Test "set -sa appends an array entry instead of replacing"
# Append an unrelated entry; the ORIGINAL must still apply afterwards. If -a
# replaced the array, or string-concatenated onto the last entry and corrupted
# it, the bars would go back to one column.
$rows = Get-Rows -Name "cpw_append" -Case "bars" -Count 15 -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2" 2>&1 | Out-Null
    & $PSMUX -L $SOCK set -sa codepoint-widths "U+4E2D=1" 2>&1 | Out-Null
}
$shape = Bar-Shape $rows
if ($shape -eq "10,5") {
    Write-Pass "the first entry still applies after an append (shape $shape)"
} else {
    Write-Fail "expected '10,5' after append, got '$shape'"
}
# ...and the appended entry took effect too: U+4E2D forced to one column now
# fits 15 on a row where it normally wraps 10 + 5.
$rows = Get-Rows -Name "cpw_append2" -Case "han" -Count 15 -Expect 0x4E2D -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2" 2>&1 | Out-Null
    & $PSMUX -L $SOCK set -sa codepoint-widths "U+4E2D=1" 2>&1 | Out-Null
}
$hanCounts = @()
foreach ($r in $rows) {
    $n = @($r.ToCharArray() | Where-Object { [int]$_ -eq 0x4E2D }).Count
    if ($n -gt 0) { $hanCounts += $n }
}
$hanShape = ($hanCounts -join ",")
if ($hanShape -eq "15") {
    Write-Pass "the appended entry narrowed U+4E2D to one column (shape $hanShape)"
} else {
    Write-Fail "expected the appended entry to give '15', got '$hanShape'"
}

Write-Test "a malformed entry is dropped without discarding the good ones"
$rows = Get-Rows -Name "cpw_bad" -Case "bars" -Count 15 -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2,U+ZZZZ=1,U+2503=9" 2>&1 | Out-Null
}
$shape = Bar-Shape $rows
if ($shape -eq "10,5") {
    Write-Pass "the valid entry survived alongside two invalid ones (shape $shape)"
} else {
    Write-Fail "expected '10,5' with junk entries present, got '$shape'"
}

# === psmux.conf ==============================================================
Write-Test "the option is honoured from psmux.conf at server start"
# Runs on its OWN socket namespace: the config is read at server start, so
# this case needs a server that has never been touched by the `set -s` cases
# above, while the anchor keeps the shared one alive.
$conf = Join-Path $work "cpw.conf"
"set -s codepoint-widths `"U+2502=2`"" | Set-Content -Path $conf -Encoding UTF8
& $PSMUX -L $SOCK2 kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$rows = Get-Rows -Name "cpw_conf" -Case "bars" -Count 15 -ServerArgs @("-f", $conf) -Sock $SOCK2
$shape = Bar-Shape $rows
if ($shape -eq "10,5") {
    Write-Pass "psmux.conf set the width before any client attached (shape $shape)"
} else {
    Write-Fail "expected '10,5' from psmux.conf, got '$shape'"
}
& $PSMUX -L $SOCK2 kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 600

# === source-file =============================================================
Write-Test "source-file applies the option to a running server"
$srcConf = Join-Path $work "cpw_source.conf"
"set -s codepoint-widths `"U+2502=2`"" | Set-Content -Path $srcConf -Encoding UTF8
$rows = Get-Rows -Name "cpw_source" -Case "bars" -Count 15 -Setup {
    & $PSMUX -L $SOCK source-file $srcConf 2>&1 | Out-Null
}
$shape = Bar-Shape $rows
if ($shape -eq "10,5") {
    Write-Pass "source-file changed the width live (shape $shape)"
} else {
    Write-Fail "expected '10,5' after source-file, got '$shape'"
}

# === erase ===================================================================
Write-Test "erasing leaves nothing behind with the override active"
# The #639 shape: a full screen program fills the pane and exits. Neither half
# of a widened glyph may survive the clear.
$rows = Get-Rows -Name "cpw_erase" -Case "erase" -Count 15 -Expect 0 -Setup {
    & $PSMUX -L $SOCK set -s codepoint-widths "U+2502=2" 2>&1 | Out-Null
}
$leftover = 0
foreach ($r in $rows) { $leftover += (Count-Bars $r) }
if ($leftover -eq 0) {
    Write-Pass "no widened glyph survived the erase"
} else {
    Write-Fail "$leftover widened glyphs were stranded after the erase"
}

# === TEARDOWN ================================================================
foreach ($c in @("anchor", "gate", "default", "control", "set", "range", "unset",
                 "append", "append2", "bad", "conf", "source", "erase")) {
    Kill-Sess "cpw_$c"
    Kill-Sess "cpw_$c" $SOCK2
}
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
& $PSMUX -L $SOCK2 kill-server 2>&1 | Out-Null
Remove-Item $work -Recurse -Force -EA SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\cpw_*" -Force -EA SilentlyContinue

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
