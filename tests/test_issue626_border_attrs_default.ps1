# Issue #626: two pane border style gaps found while reviewing PR #624.
#
#   Gap 1  `pane-border-style 'fg=red,bold'` drew red but never bold, because
#          the client rebuilt the border style from scratch and kept only the
#          colours. tmux merges the attribute half instead (style.c:459,
#          `gc->attr |= sy->gc.attr;`).
#
#   Gap 2  `set -gu pane-border-style` landed on the terminal default
#          foreground while a fresh session drew the built in grey, because the
#          option catalog advertised the literal word "default" as the value to
#          restore. tmux gives the option a real default style
#          (options-table.c:1605 `fg=themelightgrey`) so unset and startup can
#          never disagree.
#
# The border is drawn by the CLIENT, never by the pane, so `capture-pane` says
# nothing about it. This test hosts a real attached client inside a
# CreatePseudoConsole (tests/conptycap.cs, same approach as
# tests/test_issue589_undercurl.ps1) and greps the exact bytes the client
# writes to its outer terminal, so every assertion below is byte level.

$ErrorActionPreference = "Continue"
$ESC  = [char]27
$SOCK = "i626"
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

$work = Join-Path $env:TEMP "psmux_i626"
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Kill-Sess([string]$n) { & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null }

function New-Split {
    param([string]$Name)
    Kill-Sess $Name
    Start-Sleep -Milliseconds 400
    & $PSMUX -L $SOCK new-session -d -s $Name -x 100 -y 30 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    & $PSMUX -L $SOCK split-window -v -t $Name 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    & $PSMUX -L $SOCK has-session -t $Name 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

# --- The client byte capture -------------------------------------------------
# The launcher clears the psmux environment (an attached client refuses to nest)
# and clears NO_COLOR, which some agent shells export and which would strip the
# very SGR sequences under test.
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
$capExe = Join-Path $work "conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc) -and -not (Test-Path $capExe)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}

function Capture-Client {
    param([string]$Session, [int]$DrainMs = 6000)
    if (-not (Test-Path $capExe)) { return $null }
    $launch = Join-Path $work "attach_$Session.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $SOCK attach -t $Session
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = Join-Path $work "client_$Session.bin"
    Remove-Item $outBin -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "$DrainMs"
    Start-Process -FilePath $capExe -ArgumentList @($outBin,"100","30","8",$launch) -Wait -WindowStyle Minimized
    if (-not (Test-Path $outBin)) { return $null }
    return [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($outBin))
}

# Every distinct SGR run that directly precedes a box drawing glyph. This is the
# style the outer terminal actually applies to a border cell.
function Border-Sgrs {
    param([string]$Text)
    if (-not $Text) { return @() }
    $rx = [regex]"((?:$ESC\[[0-9;:]*m)+)[\u2500-\u257F]"
    $out = @()
    foreach ($m in $rx.Matches($Text)) {
        $sgr = $m.Groups[1].Value
        if ($out -notcontains $sgr) { $out += $sgr }
    }
    return $out
}

# The SGR run in force on the outer terminal when the first cell of a given row
# is written.
#
# This used to match the literal shape "<SGR><ESC[row;1H><glyph>", the style run
# sitting immediately in front of an ABSOLUTE cursor move to the row. That shape
# is not psmux's to control. These bytes are ConPTY's re-encoding of the console
# the client drew, and ConPTY only reaches a row with an absolute move when it is
# repainting a non contiguous region. Proven by capture: when the client paints
# the whole screen in ONE pass, ConPTY walks down the screen with CR/LF and emits
# no `ESC[15;1H` at all, even though the identical `ESC[90m` is in force when the
# border row is written:
#
#   two passes:  ...f7><ESC>[90m<ESC>[15;1H<50 glyphs><ESC>[32m<50 glyphs>
#   one pass:    ...<ESC>[K<CR><LF><ESC>[90m<CR><LF><50 glyphs><ESC>[32m<50 glyphs>
#
# So track the cursor and the live style and ask the terminal state, not one byte
# pattern: walk the stream, follow CUP / CR / LF, and report the style that is
# active the moment a box drawing glyph lands in column 1 of `$Row`.
function Row-Sgr {
    param([string]$Text, [int]$Row)
    if (-not $Text) { return $null }
    $rx = [regex]("(?<sgr>$ESC\[[0-9;:]*m)|(?<cup>$ESC\[(?<r>[0-9]*)(;(?<c>[0-9]*))?H)|(?<csi>$ESC\[[0-9;:?]*[A-Za-z@``])|(?<osc>$ESC\][^$ESC\a]*($ESC\\|\a))|(?<esc>$ESC.)|(?<cr>\r)|(?<lf>\n)|(?<ch>[^$ESC\r\n])")
    $r = 1; $c = 1
    $pending = ""   # SGRs emitted since the last cell was printed
    $live = ""      # the last SGR run emitted, which is still in force
    $found = $null
    foreach ($m in $rx.Matches($Text)) {
        if ($m.Groups['sgr'].Success) { $pending += $m.Value; $live = $pending; continue }
        if ($m.Groups['cup'].Success) {
            $rv = $m.Groups['r'].Value; $cv = $m.Groups['c'].Value
            $r = if ($rv) { [int]$rv } else { 1 }
            $c = if ($cv) { [int]$cv } else { 1 }
            continue
        }
        if ($m.Groups['cr'].Success) { $c = 1; continue }
        if ($m.Groups['lf'].Success) { $r++; continue }
        if ($m.Groups['ch'].Success) {
            if ($r -eq $Row -and $c -eq 1 -and $m.Value -match "[\u2500-\u257F]") {
                # A style already in force needs no fresh SGR in front of the row.
                $found = if ($pending) { $pending } else { $live }
            }
            $pending = ""
            $c++
            continue
        }
        # Every other escape sequence here leaves the cursor where it was.
    }
    return $found
}

function Vis([string]$s) { if ($null -eq $s) { return "<none>" }; return ($s -replace $ESC, '<ESC>') }

if (-not (Test-Path $capExe)) {
    Write-Skip "csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
}

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

Write-Host "=== Issue #626: pane border attributes and the unset default ===" -ForegroundColor White
Write-Host "psmux: $PSMUX"

# ---------------------------------------------------------------------------
# Baseline: what a fresh session renders. Everything else is compared to this.
# ---------------------------------------------------------------------------
Write-Test "Baseline, a fresh session with two panes"
$freshInactive = $null
if (New-Split "i626fresh") {
    $t = Capture-Client "i626fresh"
    if ($null -eq $t) {
        Write-Skip "no client capture for the baseline"
    } else {
        $freshInactive = Row-Sgr $t 15
        if ($freshInactive -eq "$ESC[90m") {
            Write-Pass "fresh inactive border is DarkGray: $(Vis $freshInactive)"
        } else {
            Write-Fail "fresh inactive border expected <ESC>[90m, got $(Vis $freshInactive)"
        }
        $sgrs = Border-Sgrs $t
        if ($sgrs -contains "$ESC[32m") {
            Write-Pass "fresh active border is Green: <ESC>[32m"
        } else {
            Write-Fail "fresh active border <ESC>[32m not found, saw: $(($sgrs | ForEach-Object { Vis $_ }) -join ' ')"
        }
    }
    Kill-Sess "i626fresh"
} else { Write-Fail "baseline session did not start" }

# ---------------------------------------------------------------------------
# Gap 1: attributes must reach the border cells.
# ---------------------------------------------------------------------------
Write-Test "Gap 1, pane-border-style 'fg=red,bold' must emit bold"
if (New-Split "i626bold") {
    & $PSMUX -L $SOCK set-option -t i626bold -g pane-border-style 'fg=red,bold' 2>&1 | Out-Null
    $shown = (& $PSMUX -L $SOCK show-options -t i626bold -g -v pane-border-style 2>&1 | Out-String).Trim()
    if ($shown -eq "fg=red,bold") { Write-Pass "show-options reports fg=red,bold" }
    else { Write-Fail "show-options reports '$shown'" }

    $t = Capture-Client "i626bold"
    if ($null -eq $t) {
        Write-Skip "no client capture for the bold border"
    } else {
        $row = Row-Sgr $t 15
        # The inbox pseudo console of Windows 10 (19045) renders bold with a
        # basic colour as the bright code (1;91 for 1;31, see #425's suites),
        # so with bold on, the bright code is that host's spelling of the red.
        $BoldBright = [Environment]::OSVersion.Version.Build -lt 22000
        if ($row -match "31m" -or ($BoldBright -and $row -match "\[1m" -and $row -match "91m")) { Write-Pass "inactive border carries the red: $(Vis $row)" }
        else { Write-Fail "inactive border lost the red: $(Vis $row)" }
        # This is the reported bug: the colour arrived, the attribute did not.
        if ($row -match "\[1m") { Write-Pass "inactive border carries the BOLD: $(Vis $row)" }
        else { Write-Fail "inactive border dropped the bold (issue #626 gap 1): $(Vis $row)" }
    }
    Kill-Sess "i626bold"
} else { Write-Fail "bold border session did not start" }

Write-Test "Gap 1, pane-active-border-style 'fg=blue,bold,underscore'"
if (New-Split "i626attrs") {
    & $PSMUX -L $SOCK set-option -t i626attrs -g pane-active-border-style 'fg=blue,bold,underscore' 2>&1 | Out-Null
    $t = Capture-Client "i626attrs"
    if ($null -eq $t) {
        Write-Skip "no client capture for the active border attributes"
    } else {
        # 1;94 is the Windows 10 inbox host's spelling of bold blue, as above.
        $blue = if ([Environment]::OSVersion.Version.Build -lt 22000) { "(34|94)m" } else { "34m" }
        $active = (Border-Sgrs $t) | Where-Object { $_ -match $blue } | Select-Object -First 1
        if (-not $active) {
            Write-Fail "active border never took the blue"
        } else {
            if ($active -match "\[1m") { Write-Pass "active border carries BOLD: $(Vis $active)" }
            else { Write-Fail "active border dropped the bold: $(Vis $active)" }
            if ($active -match "\[4m") { Write-Pass "active border carries UNDERLINE: $(Vis $active)" }
            else { Write-Fail "active border dropped the underline: $(Vis $active)" }
        }
    }
    Kill-Sess "i626attrs"
} else { Write-Fail "active attrs session did not start" }

# ---------------------------------------------------------------------------
# Gap 2: unset must land back on what startup renders.
# ---------------------------------------------------------------------------
Write-Test "Gap 2, set -g then set -gu pane-border-style"
if (New-Split "i626unset") {
    & $PSMUX -L $SOCK set-option -t i626unset -g pane-border-style 'fg=colour244,bg=colour235' 2>&1 | Out-Null
    & $PSMUX -L $SOCK set-option -t i626unset -gu pane-border-style 2>&1 | Out-Null
    $t = Capture-Client "i626unset"
    if ($null -eq $t) {
        Write-Skip "no client capture after the unset"
    } else {
        $row = Row-Sgr $t 15
        if ($row -eq "$ESC[90m") {
            Write-Pass "after -gu the inactive border is DarkGray again: $(Vis $row)"
        } else {
            Write-Fail "after -gu the inactive border is $(Vis $row), expected <ESC>[90m (issue #626 gap 2)"
        }
        if ($freshInactive -and $row -eq $freshInactive) {
            Write-Pass "unset render is byte identical to the fresh session render"
        } elseif ($freshInactive) {
            Write-Fail "unset render $(Vis $row) differs from fresh $(Vis $freshInactive)"
        }
        $sgrs = Border-Sgrs $t
        if ($sgrs -contains "$ESC[32m") { Write-Pass "after -gu the active border is still Green" }
        else { Write-Fail "after -gu the active border lost the green" }
    }
    Kill-Sess "i626unset"
} else { Write-Fail "unset session did not start" }

Write-Test "Gap 2, the same at window scope (setw -u)"
if (New-Split "i626setw") {
    & $PSMUX -L $SOCK set-window-option -t i626setw pane-border-style 'fg=colour244' 2>&1 | Out-Null
    & $PSMUX -L $SOCK set-window-option -t i626setw -u pane-border-style 2>&1 | Out-Null
    $t = Capture-Client "i626setw"
    if ($null -eq $t) {
        Write-Skip "no client capture after setw -u"
    } else {
        $row = Row-Sgr $t 15
        if ($row -eq "$ESC[90m") { Write-Pass "after setw -u the inactive border is DarkGray again: $(Vis $row)" }
        else { Write-Fail "after setw -u the inactive border is $(Vis $row), expected <ESC>[90m" }
    }
    Kill-Sess "i626setw"
} else { Write-Fail "setw session did not start" }

Write-Test "Gap 2, pane-active-border-style unset"
if (New-Split "i626aunset") {
    & $PSMUX -L $SOCK set-option -t i626aunset -g pane-active-border-style 'fg=colour214' 2>&1 | Out-Null
    & $PSMUX -L $SOCK set-option -t i626aunset -gu pane-active-border-style 2>&1 | Out-Null
    $t = Capture-Client "i626aunset"
    if ($null -eq $t) {
        Write-Skip "no client capture after the active unset"
    } else {
        $sgrs = Border-Sgrs $t
        if ($sgrs -contains "$ESC[32m") { Write-Pass "after -gu the active border is Green again" }
        else { Write-Fail "after -gu the active border is not Green: $(($sgrs | ForEach-Object { Vis $_ }) -join ' ')" }
        $row = Row-Sgr $t 15
        if ($row -eq "$ESC[90m") { Write-Pass "the inactive border is untouched by the active unset" }
        else { Write-Fail "the inactive border changed: $(Vis $row)" }
    }
    Kill-Sess "i626aunset"
} else { Write-Fail "active unset session did not start" }

# ---------------------------------------------------------------------------
# show-options must report a value that renders like the running session, both
# before any set and after the unset.
# ---------------------------------------------------------------------------
Write-Test "show-options agrees with itself across set and unset"
if (New-Split "i626show") {
    $before = (& $PSMUX -L $SOCK show-options -t i626show -g -v pane-border-style 2>&1 | Out-String).Trim()
    & $PSMUX -L $SOCK set-option -t i626show -g pane-border-style 'fg=colour244' 2>&1 | Out-Null
    $during = (& $PSMUX -L $SOCK show-options -t i626show -g -v pane-border-style 2>&1 | Out-String).Trim()
    & $PSMUX -L $SOCK set-option -t i626show -gu pane-border-style 2>&1 | Out-Null
    $after = (& $PSMUX -L $SOCK show-options -t i626show -g -v pane-border-style 2>&1 | Out-String).Trim()

    if ($during -eq "fg=colour244") { Write-Pass "set is reported: $during" }
    else { Write-Fail "set reported as '$during'" }
    if ($after -eq $before) { Write-Pass "unset reports the startup value again: '$after'" }
    else { Write-Fail "unset reports '$after', startup reported '$before'" }
    if ($after -ne "default") { Write-Pass "the restored value is a real style, not the word default" }
    else { Write-Fail "the restored value is the literal word 'default' (issue #626 gap 2)" }
    Kill-Sess "i626show"
} else { Write-Fail "show-options session did not start" }

# ---------------------------------------------------------------------------
# Win32 TUI: a visible attached client, driven and verified through the CLI.
# ---------------------------------------------------------------------------
Write-Test "Win32 TUI, a visible attached client survives both option routes"
$TUI = "i626tui"
Kill-Sess $TUI
Start-Sleep -Milliseconds 400
$launchTui = Join-Path $work "tui.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $SOCK new-session -s $TUI -x 100 -y 30
"@ | Set-Content -Path $launchTui -Encoding ASCII
$proc = Start-Process -FilePath "cmd.exe" -ArgumentList "/c",$launchTui -PassThru
Start-Sleep -Seconds 5
& $PSMUX -L $SOCK has-session -t $TUI 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Skip "TUI session did not start"
} else {
    Write-Pass "TUI session alive with a real attached client"
    & $PSMUX -L $SOCK split-window -v -t $TUI 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    & $PSMUX -L $SOCK set-option -t $TUI -g pane-border-style 'fg=red,bold' 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $v = (& $PSMUX -L $SOCK show-options -t $TUI -g -v pane-border-style 2>&1 | Out-String).Trim()
    if ($v -eq "fg=red,bold") { Write-Pass "TUI: attribute style accepted while attached: $v" }
    else { Write-Fail "TUI: attribute style reported as '$v'" }

    & $PSMUX -L $SOCK set-option -t $TUI -gu pane-border-style 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $v = (& $PSMUX -L $SOCK show-options -t $TUI -g -v pane-border-style 2>&1 | Out-String).Trim()
    if ($v -ne "default" -and $v -ne "") { Write-Pass "TUI: unset restored a real style: $v" }
    else { Write-Fail "TUI: unset left '$v'" }

    $panes = (& $PSMUX -L $SOCK list-panes -t $TUI 2>&1 | Out-String).Trim()
    if (($panes -split "`n").Count -ge 2) { Write-Pass "TUI: both panes still alive after the option churn" }
    else { Write-Fail "TUI: pane list is $panes" }
}
Kill-Sess $TUI
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# === TEARDOWN ===
foreach ($s in @("i626fresh","i626bold","i626attrs","i626unset","i626setw","i626aunset","i626show",$TUI)) {
    Kill-Sess $s
}
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Remove-Item $work -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
