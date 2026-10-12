# Discussion #776: the choosers driven key by key against tmux's mode tree
# (mode-tree.c and window-tree.c, tag 3.7b, the release the audit cites).
#
#   a) The pane's cursor stayed lit under a chooser. tmux turns it off for
#      every mode tree mode: `(*s)->mode &= ~MODE_CURSOR;` (mode-tree.c:536).
#   b) Page Up and Page Down moved a fixed 10 rows. tmux moves by the list
#      height, `for (i = 0; i < mtd->height; i++)` (mode-tree.c:1318-1333).
#   c) `q` did not close the tree or the session chooser. tmux: `case 'q':`
#      returns 1 and the mode exits (mode-tree.c:1289-1293), so do `C-g` and
#      `C-[`.
#   d) In the tree, `+` and `-` did nothing and `h`/`l` merely moved. tmux puts
#      `h` and `-` in the KEYC_LEFT arm and `l` and `+` in the KEYC_RIGHT arm
#      (mode-tree.c:1390-1412).
#   e) Another session's windows could not be expanded (no pane rows were
#      built for them), and choosing one of its window rows landed on whatever
#      window that session already had active.
#   f) `C-p`/`C-n` did not move (mode-tree.c:1301, :1307).
#   g) The in process command path answered `choose-client` with a status
#      line claiming a single client model, and `list-clients` with one made
#      up `/dev/pts/0` row.
#
# Every check is live: a real attached client, real keys through
# WriteConsoleInput (tests/injector.cs), the client's console read back with
# tests/conread.cs, and for the cursor the client's own output bytes captured
# under a pseudoconsole (tests/conptycap.cs). GetConsoleCursorInfo cannot be
# used for the cursor: inside the alternate screen the client draws on, the
# console reports bVisible=1 even after ESC[?25l (measured), so the oracle is
# the last DECTCEM the client wrote after the chooser was drawn.
#
# The suite runs in a PSMUX_DATA_DIR of its own, so the choosers only list
# sessions it made, and it never kills any process by name.

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

. "$PSScriptRoot\isolated_config.ps1"
$env:PSMUX_SESSION = ""

$script:dataDir = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_d776_" + [guid]::NewGuid().ToString('N').Substring(0, 10))
New-Item -ItemType Directory -Force $script:dataDir | Out-Null
$env:PSMUX_DATA_DIR = $script:dataDir

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

Write-Host "`n=== Discussion #776: chooser parity ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $script:dataDir"

# ---- helpers built for this suite only ----
$toolDir = Join-Path $script:dataDir "tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $toolDir "injector.exe"
$READ = Join-Path $toolDir "conread.exe"
$CAP = Join-Path $toolDir "conptycap.exe"
& $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs")  2>&1 | Out-Null
& $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs")   2>&1 | Out-Null
& $csc /nologo /optimize /out:$CAP  (Join-Path $PSScriptRoot "conptycap.cs") 2>&1 | Out-Null
$haveTools = (Test-Path $INJ) -and (Test-Path $READ) -and (Test-Path $CAP)
Add-Result "harness tools compiled" $haveTools ""
if (-not $haveTools) { exit 1 }

$S = "d776"   # 30 windows, window 0 split in two
$O = "o776"   # window 0 split in two, plus one window

& $PSMUX new-session -d -s $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
& $PSMUX split-window -t $S 2>&1 | Out-Null
for ($i = 1; $i -lt 30; $i++) { & $PSMUX new-window -d -t $S 2>&1 | Out-Null }
& $PSMUX new-session -d -s $O 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
& $PSMUX split-window -t $O 2>&1 | Out-Null
& $PSMUX new-window -d -t $O 2>&1 | Out-Null
& $PSMUX set-buffer -t $S "disc 776 buffer" 2>&1 | Out-Null
$nwin = ([string[]](& $PSMUX list-windows -t $S 2>$null)).Count
Add-Result "setup: $S has 30 windows" ($nwin -eq 30) "count=$nwin"

# Put both sessions back in their starting state before a scenario.
function Reset-State {
    & $PSMUX select-window -t "${S}:0" 2>&1 | Out-Null
    & $PSMUX select-pane -t "${S}:0.1" 2>&1 | Out-Null
    & $PSMUX select-window -t "${O}:0" 2>&1 | Out-Null
    & $PSMUX select-pane -t "${O}:0.1" 2>&1 | Out-Null
}
function Start-Client {
    Reset-State
    $p = Start-Process -FilePath $PSMUX -ArgumentList "attach", "-t", $S -PassThru
    Start-Sleep -Seconds 4
    return $p
}
function Stop-Client($p) {
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 700
}
function Send-Keys($p, $keys) {
    & $INJ $p.Id $keys | Out-Null
    Start-Sleep -Milliseconds 900
}
function Read-Screen($p) { return [string[]](& $READ $p.Id) }
function Where-Is($sess) { return (& $PSMUX display-message -t $sess -p '#{window_index}.#{pane_index}' 2>$null) }
function Client-Session { return (& $PSMUX list-clients -t $S -F '#{client_session}' 2>$null) + (& $PSMUX list-clients -t $O -F '#{client_session}' 2>$null) }

# ════════════════════════════════════════════════════════════════════
# a) the pane cursor under a chooser
# ════════════════════════════════════════════════════════════════════
Write-Host "`n--- a) cursor under the choosers ---" -ForegroundColor Cyan
$capOut = Join-Path $script:dataDir "cap.bin"
$env:CONPTYCAP_DRAIN_MS = "60000"
Reset-State
$cap = Start-Process -FilePath $CAP -ArgumentList $capOut, "120", "30", "8", "`"$PSMUX`"", "attach", "-t", $S -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 4
$capChild = $null
$m = Select-String -Path "$capOut.log" -Pattern 'childPid=(\d+)' -EA SilentlyContinue | Select-Object -First 1
if ($m) { $capChild = [int]$m.Matches[0].Groups[1].Value }

function Read-Cap {
    $fs = [IO.File]::Open($capOut, "Open", "Read", "ReadWrite")
    $ms = New-Object IO.MemoryStream; $fs.CopyTo($ms); $fs.Close()
    return [Text.Encoding]::UTF8.GetString($ms.ToArray())
}
# The cursor state the client left the host terminal in, from the bytes it
# wrote after `$from`: the later of ESC[?25h and ESC[?25l wins.
function Cursor-After($from, $marker) {
    $all = Read-Cap
    $s = if ($all.Length -gt $from) { $all.Substring($from) } else { "" }
    $show = $s.LastIndexOf("$([char]27)[?25h"); $hide = $s.LastIndexOf("$([char]27)[?25l")
    $state = if ($show -gt $hide) { "shown" } elseif ($hide -ge 0) { "hidden" } else { "unchanged" }
    return @{ State = $state; Drawn = $s.Contains($marker) }
}

if ($capChild) {
    $capProc = [PSCustomObject]@{ Id = $capChild }
    foreach ($case in @(
        @{ Name = "tree chooser";    Keys = "^b{SLEEP:500}w{SLEEP:1500}"; Marker = "choose-tree" },
        @{ Name = "session chooser"; Keys = "^b{SLEEP:500}s{SLEEP:1500}"; Marker = "choose-session" },
        @{ Name = "buffer chooser";  Keys = "^b{SLEEP:500}={SLEEP:1500}"; Marker = "choose-buffer" })) {
        Write-Test "$($case.Name): the pane cursor is hidden while it is open"
        $from = (Read-Cap).Length
        Send-Keys $capProc $case.Keys
        $r = Cursor-After $from $case.Marker
        Add-Result "$($case.Name) drawn" $r.Drawn ""
        Add-Result "$($case.Name) hides the pane cursor" ($r.State -eq "hidden") "last DECTCEM: $($r.State)"
        Write-Test "$($case.Name): Escape closes it and the cursor comes back"
        $from = (Read-Cap).Length
        Send-Keys $capProc "{ESC}{SLEEP:900}"
        $r = Cursor-After $from "~"
        Add-Result "$($case.Name) closed, cursor shown again" ($r.State -eq "shown") "last DECTCEM: $($r.State)"
    }
    try { Stop-Process -Id $capChild -Force -EA SilentlyContinue } catch {}
} else {
    Add-Result "pseudoconsole client started" $false "no child pid in $capOut.log"
}
try { Stop-Process -Id $cap.Id -Force -EA SilentlyContinue } catch {}
Start-Sleep -Milliseconds 700

# ════════════════════════════════════════════════════════════════════
# b) Page Down and Page Up move by the list height
# ════════════════════════════════════════════════════════════════════
Write-Host "`n--- b) paging by the list height ---" -ForegroundColor Cyan
$c = Start-Client
Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}"
$scr = Read-Screen $c
# Visible list rows: the session headers ("N windows") and window rows ("(N panes)").
$H = @($scr | Where-Object { $_ -match '\d+ windows|\(\d+ panes\)' }).Count
Write-Host "  list height on screen: $H rows"
Add-Result "tree list taller than 10 rows on this console" ($H -gt 10) "height=$H"
Write-Test "Page Down from window 0 moves one list height"
Send-Keys $c "{PGDN}{SLEEP:600}{ENTER}{SLEEP:900}"
$at = Where-Is $S
# Row 0 is the session, row r the window r-1; the cursor starts on row 1.
$expect = [Math]::Min(1 + $H, 30) - 1
Add-Result "Page Down lands $H rows further" ($at -eq "$expect.0") "at=$at expected=$expect.0 (10 rows would be 10.0)"
Stop-Client $c

$c = Start-Client
Write-Test "Page Up from the last row moves one list height"
Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}{END}{SLEEP:500}{PGUP}{SLEEP:600}{ENTER}{SLEEP:900}"
# Rows: 0 d776, 1..30 its windows, 31 o776, 32 and 33 its windows.
$row = [Math]::Max(33 - $H, 0)
$at = Where-Is $S
$expectUp = if ($row -ge 1 -and $row -le 30) { "$($row - 1).0" } else { "0.1" }
Add-Result "Page Up lands $H rows back" ($at -eq $expectUp) "at=$at expected=$expectUp"
Stop-Client $c

# ════════════════════════════════════════════════════════════════════
# c) q and C-g close the choosers
# ════════════════════════════════════════════════════════════════════
Write-Host "`n--- c) q closes every chooser ---" -ForegroundColor Cyan
foreach ($case in @(
    @{ Name = "tree";    Open = "^b{SLEEP:500}w{SLEEP:1500}"; Marker = "choose-tree";    Close = "q" },
    @{ Name = "session"; Open = "^b{SLEEP:500}s{SLEEP:1500}"; Marker = "choose-session"; Close = "q" },
    @{ Name = "buffer";  Open = "^b{SLEEP:500}={SLEEP:1500}"; Marker = "choose-buffer";  Close = "q" },
    @{ Name = "tree";    Open = "^b{SLEEP:500}w{SLEEP:1500}"; Marker = "choose-tree";    Close = "^g" },
    @{ Name = "session"; Open = "^b{SLEEP:500}s{SLEEP:1500}"; Marker = "choose-session"; Close = "^g" })) {
    $c = Start-Client
    Send-Keys $c $case.Open
    $open = (Read-Screen $c) -join "`n"
    Send-Keys $c "$($case.Close){SLEEP:900}"
    $after = (Read-Screen $c) -join "`n"
    $label = if ($case.Close -eq "^g") { "C-g" } else { "q" }
    Add-Result "$($case.Name) chooser: open" ($open.Contains($case.Marker)) ""
    Add-Result "$($case.Name) chooser: $label closes it" (-not $after.Contains($case.Marker)) ""
    Add-Result "$($case.Name) chooser: $label did not switch anything" ((Where-Is $S) -eq "0.1") "at=$(Where-Is $S)"
    Stop-Client $c
}

# ════════════════════════════════════════════════════════════════════
# d) + - h l expand and collapse in the tree; f) C-n C-p move
# ════════════════════════════════════════════════════════════════════
Write-Host "`n--- d) + - h l in the tree, f) C-n C-p ---" -ForegroundColor Cyan
# The cursor opens on window 0 (two panes, collapsed). Expanded, the next row
# is its first pane; collapsed, the next row is window 1.
foreach ($case in @(
    @{ Keys = "{RIGHT}";             Expect = "0.0"; What = "Right expands (control)" },
    @{ Keys = "+";                   Expect = "0.0"; What = "+ expands" },
    @{ Keys = "l";                   Expect = "0.0"; What = "l expands" },
    @{ Keys = "{RIGHT}{SLEEP:500}{LEFT}"; Expect = "1.0"; What = "Left collapses (control)" },
    @{ Keys = "{RIGHT}{SLEEP:500}-";  Expect = "1.0"; What = "- collapses" },
    @{ Keys = "{RIGHT}{SLEEP:500}h";  Expect = "1.0"; What = "h collapses" })) {
    $c = Start-Client
    Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}$($case.Keys){SLEEP:600}j{SLEEP:500}{ENTER}{SLEEP:900}"
    $at = Where-Is $S
    Add-Result "tree: $($case.What)" ($at -eq $case.Expect) "then j Enter landed at $at, expected $($case.Expect)"
    Stop-Client $c
}

$c = Start-Client
Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}^n{SLEEP:400}^n{SLEEP:400}^p{SLEEP:400}{ENTER}{SLEEP:900}"
$at = Where-Is $S
Add-Result "tree: C-n C-n C-p moves down two and up one" ($at -eq "1.0") "at=$at expected 1.0"
Stop-Client $c

# ════════════════════════════════════════════════════════════════════
# e) another session's windows expand, and choosing one lands on it
# ════════════════════════════════════════════════════════════════════
Write-Host "`n--- e) another session in the tree ---" -ForegroundColor Cyan
$c = Start-Client
# G scrolls the list to its end, where o776's rows are.
Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}G{SLEEP:700}"
$scr = Read-Screen $c
$oHead = -1
for ($i = 0; $i -lt $scr.Count; $i++) { if ($scr[$i] -match "${O}: 2 windows") { $oHead = $i } }
$oRow = if ($oHead -ge 0 -and $oHead + 1 -lt $scr.Count) { $scr[$oHead + 1] } else { "" }
Add-Result "tree: o776 window 0 carries an expand marker" ($oRow -match '\+\s+0: pwsh\* \(2 panes\)') "row: $($oRow -replace '\s+',' ')"
Stop-Client $c

$c = Start-Client
# G is o776's last window, k its window 0 (active pane 1). Expand, step onto
# its first pane, choose it.
Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}G{SLEEP:400}k{SLEEP:400}+{SLEEP:600}j{SLEEP:400}{ENTER}{SLEEP:2500}"
$cs = Client-Session
$at = Where-Is $O
Add-Result "tree: choosing o776's pane row switches the client to o776" ($cs -match $O) "client session: $cs"
Add-Result "tree: and lands on that pane (window 0, pane 0)" ($at -eq "0.0") "o776 at=$at"
Stop-Client $c

$c = Start-Client
Send-Keys $c "^b{SLEEP:500}w{SLEEP:1500}G{SLEEP:400}{ENTER}{SLEEP:2500}"
$cs = Client-Session
$at = Where-Is $O
Add-Result "tree: choosing o776's window 1 row switches to o776" ($cs -match $O) "client session: $cs"
Add-Result "tree: and lands on window 1" ($at -eq "1.0") "o776 at=$at"
Stop-Client $c

# ════════════════════════════════════════════════════════════════════
# g) in process choose-client and list-clients
# ════════════════════════════════════════════════════════════════════
Write-Host "`n--- g) in process choose-client / list-clients ---" -ForegroundColor Cyan
# confirm-before runs its command through the in process path once answered.
$c = Start-Client
$cli = ((& $PSMUX list-clients -t $S 2>$null | Select-Object -First 1) -split ':')[0]
& $PSMUX confirm-before -t $S -p "go? " "list-clients" 2>&1 | Out-Null
Start-Sleep -Milliseconds 900
Send-Keys $c "y{SLEEP:1200}"
$scr = (Read-Screen $c) -join "`n"
Add-Result "in process list-clients shows the real client" ($cli -and $scr.Contains("$cli`:")) "client=$cli"
Add-Result "in process list-clients has no made up /dev/pts/0" (-not $scr.Contains("/dev/pts/0:")) ""
Stop-Client $c

$c = Start-Client
$cli = ((& $PSMUX list-clients -t $S 2>$null | Select-Object -First 1) -split ':')[0]
& $PSMUX confirm-before -t $S -p "go? " "choose-client" 2>&1 | Out-Null
Start-Sleep -Milliseconds 900
Send-Keys $c "y{SLEEP:1200}"
$scr = (Read-Screen $c) -join "`n"
Add-Result "in process choose-client makes no single client claim" (-not $scr.Contains("single-client")) ""
Add-Result "in process choose-client lists the attached client" ($cli -and $scr.Contains("$cli`:")) "client=$cli"
Stop-Client $c

# ════════════════════════════════════════════════════════════════════
# Cleanup: only this suite's data dir and servers
# ════════════════════════════════════════════════════════════════════
foreach ($s in @($S, $O)) { & $PSMUX kill-session -t $s 2>$null | Out-Null }
Start-Sleep -Milliseconds 500
& $PSMUX kill-server 2>$null | Out-Null
Remove-PsmuxIsolatedConfig
$env:PSMUX_DATA_DIR = $null
Start-Sleep -Milliseconds 500
Remove-Item -Recurse -Force $script:dataDir -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $pass / $($pass + $fail)" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Yellow' })
foreach ($r in $results) {
    $color  = if ($r.Pass) { 'Green' } else { 'Red' }
    $status = if ($r.Pass) { 'PASS' } else { 'FAIL' }
    Write-Host "  [$status] $($r.Test) $($r.Detail)" -ForegroundColor $color
}

exit $fail
