# Issue #835: clicking a window tab in the status bar selects the wrong
# window, and the error grows towards the right (tab 4 opens tab 3, the last
# tab cannot be reached). Regression since 3.4.0.
# Every rendered tab is located on the client's REAL screen buffer (conread)
# and clicked with a native Win32 MOUSE_EVENT (click_injector), so the test
# checks exactly what a user sees and clicks. No focus needed.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SOCK = "i835_$PID"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Build-Tool($name, $src) {
    $exe = "$env:TEMP\psmux_${name}_835.exe"
    if (-not (Test-Path $exe) -or (Get-Item $exe).LastWriteTime -lt (Get-Item "$PSScriptRoot\$src").LastWriteTime) {
        $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
        if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
        & $csc /nologo /optimize /out:$exe "$PSScriptRoot\$src" 2>&1 | Out-Null
    }
    return $exe
}
$injector = Build-Tool "click_injector" "click_injector.cs"
$conread = Build-Tool "conread" "conread.cs"
$vtinj = Build-Tool "vt_mouse_injector" "vt_mouse_injector.cs"
$script:UseVT = $false
if (-not (Test-Path $injector) -or -not (Test-Path $conread)) { Write-Fail "could not compile helpers"; exit 1 }

$launchCmd = "$env:TEMP\psmux_835_launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
$(if ($env:PSMUX_DATA_DIR) { "set PSMUX_DATA_DIR=$env:PSMUX_DATA_DIR" })
if not "%~3"=="" mode con: cols=%~3 lines=%~4
"$PSMUX" -L $SOCK -f "%~2" new-session -s %1
"@ | Set-Content -Path $launchCmd -Encoding ASCII
$script:Cols = ""; $script:Lines = ""; $script:InWT = ""

$launched = @()
function Start-Scen {
    param([string]$Name,[string]$Conf,[int]$Windows)
    $argv = @($Name, $Conf)
    if ($script:Cols) { $argv += @($script:Cols, $script:Lines) }
    $since = Get-Date
    if ($script:InWT) {
        # A real Windows Terminal window (its own window, closed when the
        # client exits). WT talks to the client through ConPTY.
        $wtArgs = @('-w', "psmux835_$PID", '--size', "$($script:InWT)", 'new-tab', $launchCmd) + $argv
        Start-Process wt.exe -ArgumentList $wtArgs | Out-Null
    } else {
        # The SGR scenarios need the client on the VT input route, as a VT
        # host (WezTerm, ssh) puts it; conhost would otherwise type the bytes.
        if ($script:UseVT) { $env:PSMUX_VT_INPUT = '1' }
        $p = Start-Process -FilePath $launchCmd -ArgumentList $argv -PassThru
        $env:PSMUX_VT_INPUT = $null
        $script:launched += $p.Id
    }
    $cli = $null
    for ($i = 0; $i -lt 60 -and -not $cli; $i++) {
        Start-Sleep -Milliseconds 250
        & $PSMUX -L $SOCK has-session -t $Name 2>$null
        if ($LASTEXITCODE -eq 0) {
            # The attached client: this namespace's new-session for $Name.
            # Get-Process, not WMI: a WMI enumeration can fail machine wide.
            $cli = Get-Process psmux -EA SilentlyContinue |
                Where-Object { $_.StartTime -ge $since.AddSeconds(-1) -and $_.CommandLine -match "-L $SOCK " -and $_.CommandLine -match "new-session -s\s+$Name\b" } |
                ForEach-Object { [pscustomobject]@{ ProcessId = $_.Id } }
        }
    }
    for ($i = 1; $i -lt $Windows; $i++) { & $PSMUX -L $SOCK new-window -d -t $Name 2>&1 | Out-Null }
    Start-Sleep -Seconds 2
    if (-not $cli) { return $null }
    return @($cli)[0].ProcessId
}

function WinIdx($Name) { (& $PSMUX -L $SOCK display-message -t $Name -p '#{window_index}' 2>&1 | Out-String).Trim() }

# Locate every rendered "N:" tab start on the status row (last screen row).
[Console]::OutputEncoding = [Text.Encoding]::UTF8
function Get-Tabs($ClientPid) {
    # -g: one char per screen cell, so a match index IS the screen column.
    $rows = @(& $conread $ClientPid -u -g 2>&1)
    $status = [string]$rows[$rows.Count - 1]
    $tabs = @{}
    foreach ($m in [regex]::Matches($status, '(?<![\d])(\d+):')) {
        $n = [int]$m.Groups[1].Value
        if (-not $tabs.ContainsKey($n)) { $tabs[$n] = $m.Index }
    }
    $status = $status -replace "`u{0001}", ''
    return @{ Row = $rows.Count - 1; Text = $status; Tabs = $tabs }
}

function Run-AllTabs {
    param([string]$Label,[string]$Name,[string]$Conf,[int]$Windows,[int]$Base,[scriptblock]$Setup)
    if ($env:PSMUX_835_ONLY -and $Label -notmatch $env:PSMUX_835_ONLY) { return }
    Write-Host "`n[$Label]" -ForegroundColor Yellow
    $cpid = Start-Scen $Name $Conf $Windows
    if (-not $cpid) { Write-Fail "$Label client did not start"; return }
    if ($Setup) { & $Setup $Name; Start-Sleep -Seconds 2 }
    $s = Get-Tabs $cpid
    Write-Host "    status row $($s.Row): [$($s.Text.TrimEnd())]"
    $bad = @()
    for ($w = $Base; $w -lt $Base + $Windows; $w++) {
        if (-not $s.Tabs.ContainsKey($w)) { $bad += "tab $w not rendered"; continue }
        # Aim at the ':' character, inside the tab text.
        $x = $s.Tabs[$w] + "$w".Length
        if ($script:UseVT) {
            # What Windows Terminal sends: SGR press + release, 1-based.
            & $vtinj $cpid "\e[<0;$($x+1);$($s.Row+1)M" 1 | Out-Null
            Start-Sleep -Milliseconds 80
            & $vtinj $cpid "\e[<0;$($x+1);$($s.Row+1)m" 1 | Out-Null
        } else {
            & $injector $cpid $x $s.Row | Out-Null
        }
        Start-Sleep -Milliseconds 700
        $got = WinIdx $Name
        if ($env:PSMUX_835_VERBOSE) { Write-Host "      tab $w col $x -> $got" }
        if ($got -ne "$w") { $bad += "click col $x on tab $w gave window $got" }
    }
    if ($bad.Count -eq 0) { Write-Pass "$Label : all $Windows tabs select the window under the pointer" }
    else { foreach ($b in $bad) { Write-Fail "$Label : $b" } }
    & $PSMUX -L $SOCK kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
}

# Click EVERY column of the window list. A tab owns the cells from its "N:"
# up to the cell before the next tab's separator; the trailing flag/space cell
# of a tab is still that tab (tmux range covers the whole formatted text).
function Run-EveryColumn {
    param([string]$Label,[string]$Name,[string]$Conf,[int]$Windows)
    if ($env:PSMUX_835_ONLY -and $Label -notmatch $env:PSMUX_835_ONLY) { return }
    Write-Host "`n[$Label]" -ForegroundColor Yellow
    $cpid = Start-Scen $Name $Conf $Windows
    if (-not $cpid) { Write-Fail "$Label client did not start"; return }
    $s = Get-Tabs $cpid
    Write-Host "    status row $($s.Row): [$($s.Text.TrimEnd())]"
    # Only window numbers: the status-right clock (" %H:%M ") also reads as
    # "N:" and, between 10:00 and 23:59, as a tab number past the last window.
    $starts = @($s.Tabs.GetEnumerator() | Where-Object { $_.Key -lt $Windows } | Sort-Object Value)
    $bad = 0; $checked = 0
    for ($k = 0; $k -lt $starts.Count; $k++) {
        $w = $starts[$k].Key
        $from = $starts[$k].Value
        # tab text = "N:pwsh" + flag cell; the separator cell after it is a gap.
        $to = if ($k + 1 -lt $starts.Count) { $starts[$k + 1].Value - 2 } else { $from + "$($w):pwsh".Length }
        for ($x = $from; $x -le $to; $x++) {
            # Start from a window that is not $w so a miss is visible.
            $other = if ($w -eq 0) { 1 } else { 0 }
            & $PSMUX -L $SOCK select-window -t "${Name}:$other" 2>&1 | Out-Null
            if ($script:UseVT) {
                & $vtinj $cpid "\e[<0;$($x+1);$($s.Row+1)M" 1 | Out-Null
                Start-Sleep -Milliseconds 40
                & $vtinj $cpid "\e[<0;$($x+1);$($s.Row+1)m" 1 | Out-Null
            } else {
                & $injector $cpid $x $s.Row | Out-Null
            }
            Start-Sleep -Milliseconds 350
            $got = WinIdx $Name
            $checked++
            if ($got -ne "$w") { $bad++; Write-Fail "$Label : col $x is tab $w, click gave $got" }
        }
    }
    if ($bad -eq 0) { Write-Pass "$Label : all $checked cells of $($starts.Count) tabs select their own window" }
    & $PSMUX -L $SOCK kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
}

$confDefault = "$env:TEMP\psmux_835_default.conf"
"set -g mouse on" | Set-Content -Path $confDefault -Encoding UTF8
$confMinimal = "$env:TEMP\psmux_835_minimal.conf"
@'
set -g mouse on
set -g status-left " #S "
set -g status-right " %H:%M "
'@ | Set-Content -Path $confMinimal -Encoding UTF8
$confBase1 = "$env:TEMP\psmux_835_base1.conf"
@'
set -g mouse on
set -g base-index 1
'@ | Set-Content -Path $confBase1 -Encoding UTF8

$confFmt = "$env:TEMP\psmux_835_fmt.conf"
@'
set -g mouse on
set -g status-format[0] "#[align=left]#[range=left] #S #[norange default]#{W:#[range=window|#{window_index}]#I:#W#{?window_flags,#{window_flags}, }#[norange default]#{?loop_last_flag,,#{window-status-separator}},#[range=window|#{window_index}]#I:#W#{?window_flags,#{window_flags}, }#[norange default]#{?loop_last_flag,,#{window-status-separator}}}#[align=right] %H:%M "
'@ | Set-Content -Path $confFmt -Encoding UTF8

Write-Host "`n=== Issue #835 Tests: status tab clicks across 12 windows ===" -ForegroundColor Cyan
$before = (& $PSMUX ls 2>&1 | Out-String)

Run-AllTabs "default bar, 12 windows" "s835a" $confDefault 12 0
Run-AllTabs "reporter's minimal bar, 12 windows" "s835b" $confMinimal 12 0
Run-AllTabs "base-index 1, 12 windows" "s835c" $confBase1 12 1

# Real sessions carry flags: a last window (-), activity (#), a zoomed window (Z).
Run-AllTabs "mixed window flags, 12 windows" "s835d" $confDefault 12 0 {
    param($n)
    & $PSMUX -L $SOCK set -g monitor-activity on 2>&1 | Out-Null
    & $PSMUX -L $SOCK split-window -d -t "${n}:5" 2>&1 | Out-Null
    & $PSMUX -L $SOCK resize-pane -Z -t "${n}:5" 2>&1 | Out-Null
    & $PSMUX -L $SOCK select-window -t "${n}:3" 2>&1 | Out-Null
    & $PSMUX -L $SOCK select-window -t "${n}:0" 2>&1 | Out-Null
    & $PSMUX -L $SOCK send-keys -t "${n}:8" "echo act" Enter 2>&1 | Out-Null
    & $PSMUX -L $SOCK send-keys -t "${n}:9" "echo act" Enter 2>&1 | Out-Null
}

# Names outside ASCII, the way automatic-rename or a user rename produces them.
Run-AllTabs "non-ASCII window names, 12 windows" "s835e" $confDefault 12 0 {
    param($n)
    $names = @("caf$([char]0xe9)", "$([char]0x65e5)$([char]0x672c)", "build", "$([char]0x2192)log", "srv", "x", "longer-name-here", "db", "w8", "w9", "w10", "w11")
    for ($i = 0; $i -lt 12; $i++) { & $PSMUX -L $SOCK rename-window -t "${n}:$i" $names[$i] 2>&1 | Out-Null }
}

Run-EveryColumn "every cell, reporter's bar, 12 windows" "s835k" $confMinimal 12

# The reporter's host: Windows Terminal, 319x81.
if (Get-Command wt.exe -EA SilentlyContinue) {
    $script:InWT = "319,81"
    Run-EveryColumn "Windows Terminal 319x81, every cell, 12 windows" "s835w" $confMinimal 12
    $script:InWT = ""
}

# A tmux style status-format[0] window list (#{W:} loop with ranges).
Run-AllTabs "status-format W loop, 12 windows" "s835j" $confFmt 12 0

# The reporter's client: 319x81.
$script:Cols = "319"; $script:Lines = "81"
Run-AllTabs "319x81 client, 12 windows" "s835f" $confMinimal 12 0
$script:Cols = ""; $script:Lines = ""

# VT input route (WezTerm, ssh, PSMUX_VT_INPUT=1): SGR reports arrive in the input stream.
$script:UseVT = $true
Run-AllTabs "VT (SGR) clicks, 12 windows" "s835g" $confMinimal 12 0
Run-AllTabs "VT (SGR) clicks, non-ASCII names" "s835h" $confDefault 12 0 {
    param($n)
    $names = @("caf$([char]0xe9)", "$([char]0x65e5)$([char]0x672c)", "build", "$([char]0x2192)log", "srv", "x", "longer-name-here", "db", "w8", "w9", "w10", "w11")
    for ($i = 0; $i -lt 12; $i++) { & $PSMUX -L $SOCK rename-window -t "${n}:$i" $names[$i] 2>&1 | Out-Null }
}
$script:UseVT = $false

# The reporter's real config (issue comment 3): base-index 1, padded
# window-status formats with #[fg=...,bold] style tokens and a conditional that
# adds " <flags>" only to flagged windows (so tab widths change as the current
# and last window move), status-left with U+258C and #{b:pane_current_path}
# under status-left-length 80, status-right with U+2325 U+2261 U+2191 U+2590
# under status-right-length 220, and the reporter's ten window names (spaces,
# 'Workbench' last (-), 'mfb' current (*)). Every cell of every tab is clicked,
# leading pad, number, name, flag and trailing pad, each from a state where
# another window is current, and the tab is located again before every click
# because its neighbours' flags move it.
$confReporter = "$env:TEMP\psmux_835_reporter.conf"
$reporterConf = @(
    'set -g base-index 1'
    'set -g pane-base-index 1'
    'set -g mouse on'
    'set -g pwsh-mouse-selection off'
    'set -g mouse-selection on'
    'set -g status-position bottom'
    'set -g status-justify left'
    'set -g status-interval 15'
    'set -g status-left-length 80'
    'set -g status-right-length 220'
    "set -g status-left `"#[fg=#1a1b26,bg=#7aa2f7,bold] $([char]0x258C) #S #[fg=#7aa2f7,bg=#292e42,nobold] #{b:pane_current_path} #[default] `""
    "set -g status-right `"#[fg=#565f89] $([char]0x2325) #{session_windows} $([char]0x2261) #{window_panes} $([char]0x2191) %H:%M #[fg=#7aa2f7]$([char]0x2590)`""
    'set -g window-status-format "#[fg=#565f89]  #I #W#{?window_flags, #{window_flags},}  "'
    'set -g window-status-current-format "#[fg=#9ece6a,bold]  #I #W#{?window_flags, #{window_flags},}  "'
) -join "`n"
[IO.File]::WriteAllText($confReporter, $reporterConf + "`n", (New-Object Text.UTF8Encoding $false))
$reporterNames = @("MFB Cli", "EventBus Refactoring", "Brian Ops", "BodProcessor Refactoring", "DevOps",
    "DigiSupportApp", "DevOps ERP", "Workbench", "CallCentreAPI", "mfb")

function Find-ReporterTab($ClientPid, [int]$Idx, [string]$TabName) {
    $s = Get-Tabs $ClientPid
    # "  N name" then an optional " <flags>" then the two cell trailing pad.
    $m = [regex]::Match($s.Text, "  $Idx " + [regex]::Escape($TabName) + '( [^ ]+)?  ')
    if (-not $m.Success) { return $null }
    return @{ Row = $s.Row; Start = $m.Index; End = $m.Index + $m.Length - 1; Text = $s.Text }
}

function Run-Reporter {
    param([string]$Label,[string]$Name)
    if ($env:PSMUX_835_ONLY -and $Label -notmatch $env:PSMUX_835_ONLY) { return }
    Write-Host "`n[$Label]" -ForegroundColor Yellow
    $cpid = Start-Scen $Name $confReporter 10
    if (-not $cpid) { Write-Fail "$Label client did not start"; return }
    $idx = @((& $PSMUX -L $SOCK list-windows -t $Name -F '#{window_index}' 2>&1) | ForEach-Object { "$_".Trim() })
    if ($idx.Count -ne 10 -or $idx[0] -ne '1') { Write-Fail "$Label : expected windows 1..10, got [$($idx -join ',')]"; return }
    for ($i = 0; $i -lt 10; $i++) { & $PSMUX -L $SOCK rename-window -t "${Name}:$($idx[$i])" $reporterNames[$i] 2>&1 | Out-Null }
    & $PSMUX -L $SOCK select-window -t "${Name}:8" 2>&1 | Out-Null
    & $PSMUX -L $SOCK select-window -t "${Name}:10" 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $first = Find-ReporterTab $cpid 1 $reporterNames[0]
    if ($first) { Write-Host "    status row $($first.Row): [$($first.Text.TrimEnd())]" }
    $bad = 0; $checked = 0
    for ($k = 0; $k -lt 10; $k++) {
        $w = [int]$idx[$k]
        $other = if ($w -eq 1) { 2 } else { 1 }
        for ($o = 0; $o -lt 64; $o++) {
            & $PSMUX -L $SOCK select-window -t "${Name}:$other" 2>&1 | Out-Null
            Start-Sleep -Milliseconds 150
            $tab = Find-ReporterTab $cpid $w $reporterNames[$k]
            if (-not $tab) {
                # A console host narrower than the reporter's 319 columns
                # (conhost stops at the screen width) cuts the list short.
                $cw = [int]((& $PSMUX -L $SOCK display -p -t $Name '#{client_width}' 2>&1 | Out-String).Trim())
                if ($cw -lt 240) { Write-Host "    [SKIP] tab $w is past the edge of a $cw column client" -ForegroundColor DarkYellow }
                else { $bad++; Write-Fail "$Label : tab $w ($($reporterNames[$k])) not rendered" }
                break
            }
            $x = $tab.Start + $o
            if ($x -gt $tab.End) { break }
            if ($script:UseVT) {
                & $vtinj $cpid "\e[<0;$($x+1);$($tab.Row+1)M" 1 | Out-Null
                Start-Sleep -Milliseconds 40
                & $vtinj $cpid "\e[<0;$($x+1);$($tab.Row+1)m" 1 | Out-Null
            } else {
                & $injector $cpid $x $tab.Row | Out-Null
            }
            Start-Sleep -Milliseconds 350
            $got = WinIdx $Name
            $checked++
            if ($got -ne "$w") { $bad++; Write-Fail "$Label : col $x is tab $w ($($reporterNames[$k])), click gave $got" }
        }
    }
    if ($bad -eq 0) { Write-Pass "$Label : all $checked cells of 10 tabs select their own window" }
    & $PSMUX -L $SOCK kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
}

$script:Cols = "319"; $script:Lines = "81"
Run-Reporter "reporter's config, classic console" "s835r"
$script:Cols = ""; $script:Lines = ""
if (Get-Command wt.exe -EA SilentlyContinue) {
    $script:InWT = "319,81"
    Run-Reporter "reporter's config, Windows Terminal 319x81" "s835s"
    $script:InWT = ""
}
$script:UseVT = $true
Run-Reporter "reporter's config, VT (SGR) clicks" "s835t"
$script:UseVT = $false

# === TEARDOWN ===
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
foreach ($p in $launched) { Stop-Process -Id $p -Force -EA SilentlyContinue }
Remove-Item "$psmuxDir\${SOCK}__*" -Force -EA SilentlyContinue
Remove-Item "$env:TEMP\psmux_835_*.conf" -Force -EA SilentlyContinue
$after = (& $PSMUX ls 2>&1 | Out-String)
if ($before -ne $after) { Write-Fail "default namespace changed during the test" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
