# Issue #767 follow up: three things a floating pane is still outside of after
# the fix in 1633ca90, which covered prefix *, list-panes and the formats, the
# cursor, and the confirm box.
#
# tmux reaches both renames through `command-prompt`: `,` is
# `command-prompt -I'#W' { rename-window -- '%%' }` and `$` is
# `command-prompt -I'#S' { rename-session -- '%%' }` (key-bindings.c:368 and
# :361, tag 3.7c). `-I` puts the current name in, the status prompt draws a
# cursor, and prompt.c gives it the whole line editor. psmux draws its own
# overlay instead, which until this change started empty, showed no cursor and
# took only Backspace.
#
# GROUND TRUTH: the text of the attached client's console (tests\conread.cs)
# for what the overlay holds, and its console cursor
# (GetConsoleScreenBufferInfo through tests\cursorprobe.cs) for where typing
# will land. Keys go in through tests\injector.cs.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null
# NO_COLOR makes the client draw without colour, and then no highlight can be
# seen in the console attributes at all.
Remove-Item Env:NO_COLOR -EA SilentlyContinue

$NS   = "rp-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "rp"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cr_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }
function D($f) { ((P display-message -t $SESS -p $f) -join '').Trim() }

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
foreach ($tool in "conread", "injector", "cursorprobe") {
    $exe = Join-Path $TMP "$tool.exe"
    if ($csc -and (Test-Path $csc)) {
        & $csc /nologo /optimize /out:$exe (Join-Path $PSScriptRoot "$tool.cs") 2>&1 | Out-Null
    }
    if (-not (Test-Path $exe)) {
        Write-Fail "could not build tests\$tool.cs (csc.exe unavailable)"
        Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
        exit 1
    }
}
$RD  = Join-Path $TMP "conread.exe"
$INJ = Join-Path $TMP "injector.exe"
$CUR = Join-Path $TMP "cursorprobe.exe"

function Stop-Opened {
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
}

function Kill-Rig {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Stop-Opened
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

$CONF = Join-Path $TMP "cr.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell $POWERSHELL",
    "set -g mode-keys vi",
    "set -g history-limit 2000",
    "set -g status-left ''"
) | Set-Content -Path $CONF -Encoding ASCII

function Start-Attached {
    $p = Start-Process -FilePath $PSMUX `
        -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s",$SESS,"-x","100","-y","30" -PassThru
    $script:Opened += $p.Id
    $portFile = Join-Path $psmuxDir "${NS}__${SESS}.port"
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 250
        if (Test-Path $portFile) {
            $port = (Get-Content $portFile -Raw).Trim()
            try {
                $t = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $t.Close()
                Start-Sleep -Milliseconds 2500
                return $p
            } catch {}
        }
    }
    return $null
}

function Inj($keys) { & $INJ $script:cpid $keys | Out-Null; Start-Sleep -Milliseconds 450 }

function Screen([switch]$Attr) {
    $o = Join-Path $TMP "screen.txt"
    $a = @("$script:cpid"); if ($Attr) { $a += "-a" }
    Start-Process -FilePath $RD -ArgumentList $a -Wait -WindowStyle Hidden -RedirectStandardOutput $o | Out-Null
    if (Test-Path $o) { return @(Get-Content $o) }
    return @()
}

# The overlay row: the one that reads "name: ...". Returns the row, the column
# the typed text starts at, and the line.
function Find-Name($screen) {
    for ($i = 0; $i -lt $screen.Count; $i++) {
        $line = [string]$screen[$i]
        $at = $line.IndexOf("name: ")
        if ($at -ge 0) { return @{ Row = $i; After = $at + 6; Line = $line } }
    }
    return $null
}

# What the overlay HOLDS, exactly. The box border comes back from the screen
# reader as non ASCII, so it is dropped before the trim. Compared with -eq, not
# -match: `name: rp` matches `name: rp-4f2a1c__rp` too, and that is the bug
# issue #759 is about.
function Name-Value($nm) {
    if (-not $nm) { return $null }
    $tail = [string]$nm.Line
    if ($nm.After -ge $tail.Length) { return "" }
    return ($tail.Substring($nm.After) -replace '[^ -~]', ' ').Trim()
}

function Overlay-Title($screen) {
    foreach ($l in $screen) {
        $s = [string]$l
        if ($s -match 'rename (session|window)') { return $Matches[0] }
    }
    return ""
}

function Cursor {
    $o = Join-Path $TMP ("cur_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".json")
    Start-Process -FilePath $CUR -ArgumentList "$($script:cpid)", "`"$o`"", "3", "60", "1" `
        -Wait -WindowStyle Hidden | Out-Null
    if (-not (Test-Path $o)) { return $null }
    $j = Get-Content $o -Raw -Encoding UTF8 | ConvertFrom-Json
    Remove-Item $o -Force -EA SilentlyContinue
    return $j
}

# ── Rig ──

Kill-Rig
$proc = Start-Attached
if (-not $proc) {
    Write-Fail "the attached client never came up"
    Kill-Rig
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
    exit 1
}
$script:cpid = $proc.Id
Start-Sleep -Milliseconds 800
& $PSMUX -L $NS rename-window -t $SESS "alpha" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Write-Info "the window is called [$((( & $PSMUX -L $NS list-windows -a -F '#{window_name}') -join '').Trim())]"

# ── 1. a paste goes into the float, not the shell behind it ──
#
# Typed text reaches the float; Ctrl+V did not. `send_text_to_active` and
# `send_bytes_to_active` route to a focused float and the paste path did not.

function Floats { (((P dump-state) -join '') -split '"floats":\[\{').Count - 1 }

Write-Head "1. Ctrl+V pastes into the float"
Inj "^b{SLEEP:400}*"
Start-Sleep -Milliseconds 4000
Check ((Floats) -eq 1) "prefix * opened a float" "no float"

Set-Clipboard -Value "PASTEMARK"
Start-Sleep -Milliseconds 400
Inj "^v"
Start-Sleep -Milliseconds 1500
$pr = Screen
$mark = -1
for ($i = 0; $i -lt $pr.Count; $i++) { if (([string]$pr[$i]).IndexOf("PASTEMARK") -ge 0) { $mark = $i; break } }
Write-Info ("PASTEMARK landed on row {0}" -f $mark)
Write-Info ("row 0, the pane behind: [{0}]" -f ([string]$pr[0]).Trim())
Check ($mark -gt 0) "the paste is in the float" "it is on row $mark, the pane behind"
Check ((([string]$pr[0]).IndexOf("PASTEMARK")) -lt 0) "nothing leaked to the pane behind" `
    "row 0 reads [$(([string]$pr[0]).Trim())]"

# ── 2. plain list-windows counts the float ──
#
# `#{window_panes}` counts it since 1633ca90; the text form has its own
# counter. tmux prints "(2 panes)" for one tiled pane and one float.

Write-Head "2. list-windows counts the float"
$lw = ((P list-windows) -join ' ').Trim()
Write-Info ("list-windows: {0}" -f $lw)
Check ($lw -match '\(2 panes\)') "list-windows says 2 panes" "it reads [$lw]"
$wp = ((P list-windows -F '#{window_panes}') -join ' ').Trim()
Check ($wp -match '^2') "and #{window_panes} agrees" "it reads [$wp]"

# ── 3. prefix w lists the float ──
#
# The choosers are built from a tree the server sends, and that tree is a
# third list again: it carried only the tiled panes, so the float had no row
# and the window there was one pane short.

Write-Head "3. choose-tree counts the float"
Inj "^b{SLEEP:400}w"
Start-Sleep -Milliseconds 1200
$row = ""
foreach ($l in (Screen)) { $s2 = [string]$l; if ($s2 -match '\(\d+ panes\)') { $row = $s2.Trim(); break } }
Write-Info ("the window row reads [{0}]" -f $row)
Check ($row -match '\(2 panes\)') "choose-tree counts the float" "it reads [$row]"
Inj "{ESC}"
Start-Sleep -Milliseconds 500

# ── Cleanup ──

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:Skipped) skipped ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
