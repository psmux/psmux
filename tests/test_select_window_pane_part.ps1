# select-window with a pane part in its target.
#
# tmux's select-window uses the target's window alone: cmd-select-window.c
# takes `target->wl` and calls session_select, and never touches the pane.
# The pane part only has to exist in that window (cmd-find.c:1189-1217), a
# %id of another window is "can't find pane" (cmd-find.c:592-598), and a bare
# %id names the window holding it (cmd-find.c:1103-1104). Measured on tmux
# 3.4: `select-window -t X:1.1` makes window 1 current and leaves every
# window's active pane as it was.
#
# psmux focused the pane part as well, before it switched windows: in the
# CURRENT window for a number, so `-t X:1.1` left window 0 on pane 1, and in
# the pane's own window for a %id. A pane part that named no pane was never
# checked, so `-t 'X:{end}.9'` switched windows at exit 0, and
# `-t 'X:{nosuch}.1'` exited 1 having moved window 0's pane.
#
# -n, -p and -l move relative to the current window, but tmux still resolves
# the target first and refuses a miss before anything moves (measured on 3.4:
# `select-window -n -t 'X:{end}.99'` is "can't find pane: 99" at exit 1);
# psmux moved at exit 0. Session Y (three windows) tells the three moves
# apart and session Z (one window) has no last window.
#
# Each case resets the focus (window 0 current, window 1 last, the first pane
# active in both), checks the reset against the state recorded at the start,
# runs one command and compares the whole state: every pane's index, id,
# pane_active and window_active, and every window's active and last flags.
# It runs once with pane-base-index 0 and once with 1. The same targets go
# through the CLI, a control client, and a hook (the server's own command
# path, where select-window took the window part only and never checked the
# pane, and a bare %id selected nothing).
#
# Isolation: a unique -L namespace per run, a PSMUX_DATA_DIR, USERPROFILE and
# HOME of its own, a config of one line, and cleanup only with
# `-L <ns> kill-server`. The default namespace's `ls` is compared before and
# after. Any mismatch, timeout, setup failure or cleanup failure exits
# non-zero.
#
# PSMUX_TEST_EXE picks the binary (default: psmux on PATH). PSMUX_TEST_ROOT
# picks where the scratch folder goes (default: the system temp folder).

$ErrorActionPreference = "Stop"
$script:Passed = 0
$script:Failed = 0
function Pass($Message) { Write-Host "  [PASS] $Message" -ForegroundColor Green; $script:Passed++ }
function Fail($Message) { Write-Host "  [FAIL] $Message" -ForegroundColor Red; $script:Failed++ }
function Check($Ok, $Message, $Detail) { if ($Ok) { Pass $Message } else { Fail "$Message -> $Detail" } }

$Psmux = $env:PSMUX_TEST_EXE
if (-not $Psmux) { $Found = Get-Command psmux -ErrorAction SilentlyContinue; if ($Found) { $Psmux = $Found.Source } }
if (-not $Psmux) { Write-Host "psmux binary not found"; exit 2 }
$Psmux = (Resolve-Path $Psmux).Path

$RootBase = if ($env:PSMUX_TEST_ROOT) { $env:PSMUX_TEST_ROOT } else { [System.IO.Path]::GetTempPath() }
$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$Root = Join-Path $RootBase ("psmux_swp_" + $Tag)

$TouchedNames = @('PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'USERPROFILE', 'HOME', 'TMUX', 'TMUX_PANE',
    'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL',
    'PSMUX_SERVER_MODE', 'PSMUX_REMOTE_ATTACH')
$SavedEnv = @{}
foreach ($Name in $TouchedNames) { $SavedEnv[$Name] = [Environment]::GetEnvironmentVariable($Name) }

function Restore-Env {
    foreach ($Name in $TouchedNames) {
        if ($null -eq $SavedEnv[$Name]) { Remove-Item "Env:$Name" -ErrorAction SilentlyContinue }
        else { Set-Item "Env:$Name" $SavedEnv[$Name] }
    }
}

function Get-DefaultLs {
    $Output = & $Psmux ls 2>&1 | ForEach-Object { "$_" }
    return ($Output -join "`n").Trim()
}

# Runs the binary in the test namespace with a time limit. A timeout kills
# only that process (by its own handle) and counts as a failure.
function Invoke-Psmux([string[]]$CliArgs, [int]$TimeoutMs = 15000) {
    $Info = New-Object System.Diagnostics.ProcessStartInfo
    $Info.FileName = $Psmux
    foreach ($Arg in @("-L", $script:Ns) + $CliArgs) { $Info.ArgumentList.Add($Arg) }
    $Info.UseShellExecute = $false
    $Info.RedirectStandardOutput = $true
    $Info.RedirectStandardError = $true
    $Process = [System.Diagnostics.Process]::Start($Info)
    $StdoutTask = $Process.StandardOutput.ReadToEndAsync()
    $StderrTask = $Process.StandardError.ReadToEndAsync()
    if (-not $Process.WaitForExit($TimeoutMs)) {
        $Process.Kill()
        $Process.WaitForExit()
        throw "timed out: $($CliArgs -join ' ')"
    }
    $Process.WaitForExit()
    return @{ Code = $Process.ExitCode; Out = $StdoutTask.Result.Trim(); Err = $StderrTask.Result.Trim() }
}

function Expect-Step([string[]]$CliArgs) {
    $Result = Invoke-Psmux $CliArgs
    if ($Result.Code -ne 0) { throw "step '$($CliArgs -join ' ')' failed: exit $($Result.Code), $($Result.Err)" }
    return $Result
}

function Start-Session([string]$Name) {
    & $Psmux -L $script:Ns new-session -d -s $Name *> $null
    $Deadline = (Get-Date).AddSeconds(15)
    do {
        $Probe = Invoke-Psmux @("has-session", "-t", $Name) 5000
        if ($Probe.Code -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $Deadline)
    foreach ($PidFile in Get-ChildItem $script:DataDir -Filter "*.pid" -ErrorAction SilentlyContinue) {
        $script:ServerPids += [int](([System.IO.File]::ReadAllText($PidFile.FullName)).Trim().Split(":")[0])
    }
    if ($Probe.Code -ne 0) { throw "session $Name did not come up" }
}

# Panes "<window index>|<pane id>|<pane index>|<pane active>|<window active>"
# sorted, then windows "<index>:<active>:<last>".
function Get-State([string]$Session = "X") {
    $Panes = ((Expect-Step @("list-panes", "-s", "-t", $Session, "-F", "#{window_index}|#{pane_id}|#{pane_index}|#{pane_active}|#{window_active}")).Out -split "`r?`n" | Sort-Object) -join " "
    $Windows = ((Expect-Step @("list-windows", "-t", $Session, "-F", "#{window_index}:#{window_active}:#{window_last_flag}")).Out -split "`r?`n") -join " "
    return "$Panes || $Windows"
}

# The start state with window $Current current and window $Last last; the
# active panes do not change.
function Get-WantState([int]$Current, [int]$Last, [string]$Baseline = $script:Baseline) {
    $PanePart, $WindowPart = $Baseline -split " \|\| "
    $Panes = ($PanePart -split " " | ForEach-Object {
            $Field = $_.Split("|")
            $Field[4] = if ([int]$Field[0] -eq $Current) { "1" } else { "0" }
            $Field -join "|"
        }) -join " "
    $Windows = ($WindowPart -split " " | ForEach-Object {
            $Index = [int]($_.Split(":")[0])
            "${Index}:$(if ($Index -eq $Current) { 1 } else { 0 }):$(if ($Index -eq $Last) { 1 } else { 0 })"
        }) -join " "
    return "$Panes || $Windows"
}

# Window 0 current and window 1 last, the first pane active in both.
function Reset-Focus {
    Expect-Step @("select-window", "-t", $script:W1) | Out-Null
    Expect-Step @("select-pane", "-t", $script:W1First) | Out-Null
    Expect-Step @("select-window", "-t", $script:W0) | Out-Null
    Expect-Step @("select-pane", "-t", $script:W0First) | Out-Null
    $State = Get-State
    if ($State -cne $script:Baseline) { Fail "reset did not restore the start state: '$State' (want '$($script:Baseline)')"; return $false }
    return $true
}

function Test-Select([string]$Label, [string[]]$CliArgs, [int]$WantCode, [string]$WantErr, [int]$Current, [int]$Last) {
    if (-not (Reset-Focus)) { Fail "$Label -> skipped, reset failed"; return }
    $Result = Invoke-Psmux $CliArgs
    $State = Get-State
    $Want = Get-WantState $Current $Last
    $ErrOk = if ($WantErr) { $Result.Err -match [regex]::Escape($WantErr) } else { $Result.Err -eq "" }
    Check ($Result.Code -eq $WantCode -and $ErrOk -and $State -ceq $Want) "$Label ($($CliArgs -join ' '))" "exit $($Result.Code), err '$($Result.Err)', state '$State' (want exit $WantCode, '$Want')"
}

# A control client on session X. Each command must end its own
# %begin/%end or %error block within the time limit; stdout is read one line
# at a time with a deadline, so a missing block is a failure, not a hang.
function Start-Control {
    $Info = New-Object System.Diagnostics.ProcessStartInfo
    $Info.FileName = $Psmux
    foreach ($Arg in @("-L", $script:Ns, "-C", "attach-session", "-t", "X")) { $Info.ArgumentList.Add($Arg) }
    $Info.UseShellExecute = $false
    $Info.RedirectStandardInput = $true
    $Info.RedirectStandardOutput = $true
    $Info.RedirectStandardError = $true
    $script:Cc = [System.Diagnostics.Process]::Start($Info)
    $script:CcErr = $script:Cc.StandardError.ReadToEndAsync()
    $script:CcLine = $null
    $Ready = Send-Control "display-message -p swpready" 10000
    if ($Ready.Kind -ne "end" -or $Ready.Body -notcontains "swpready") { throw "control client did not attach: $($Ready.Kind) '$($Ready.Body -join ' / ')'" }
}

function Read-ControlLine([datetime]$Deadline) {
    if ($null -eq $script:CcLine) { $script:CcLine = $script:Cc.StandardOutput.ReadLineAsync() }
    $Left = [int]($Deadline - (Get-Date)).TotalMilliseconds
    if ($Left -le 0 -or -not $script:CcLine.Wait($Left)) { return $null }
    $Line = $script:CcLine.Result
    $script:CcLine = $null
    if ($null -eq $Line) { throw "control client closed its output" }
    return $Line
}

# Sends one command and returns the block it produced: Kind "end", "error"
# or "timeout", and the lines inside the block.
function Send-Control([string]$Command, [int]$TimeoutMs = 8000) {
    $script:Cc.StandardInput.WriteLine($Command)
    $script:Cc.StandardInput.Flush()
    $Deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    $InBlock = $false
    $Body = @()
    while ($true) {
        $Line = Read-ControlLine $Deadline
        if ($null -eq $Line) { return @{ Kind = "timeout"; Body = $Body } }
        if (-not $InBlock) {
            if ($Line -match '^%begin ') { $InBlock = $true; $Body = @() }
            continue
        }
        if ($Line -match '^%end ') {
            if ($Command -match 'swpready' -and $Body -notcontains 'swpready') { $InBlock = $false; continue }
            return @{ Kind = "end"; Body = $Body }
        }
        if ($Line -match '^%error ') { return @{ Kind = "error"; Body = $Body } }
        $Body += $Line
    }
}

function Stop-Control {
    if ($null -eq $script:Cc) { return }
    $Process = $script:Cc
    $script:Cc = $null
    try { $Process.StandardInput.Close() } catch {}
    if (-not $Process.WaitForExit(5000)) {
        Fail "cleanup: control client did not exit within 5s after stdin closed"
        try { $Process.Kill() } catch {}
        if (-not $Process.WaitForExit(5000)) { Fail "cleanup: control client still running after Kill" }
    }
}

function Test-ControlSelect([string]$Label, [string]$Command, [string]$WantKind, [int]$Current, [int]$Last) {
    if (-not (Reset-Focus)) { Fail "$Label -> skipped, reset failed"; return }
    $Block = Send-Control $Command
    $State = Get-State
    $Want = Get-WantState $Current $Last
    Check ($Block.Kind -eq $WantKind -and $State -ceq $Want) "$Label (control: $Command)" "block $($Block.Kind) '$($Block.Body -join ' / ')', state '$State' (want $WantKind, '$Want')"
}

function Invoke-Suite([int]$Base) {
    $P1 = $Base; $P2 = $Base + 1; $Pmiss = $Base + 5

    Write-Host "`n--- pane-base-index ${Base}: one window request, the pane only checked ---" -ForegroundColor Yellow
    Test-Select "1 another window's pane by number: window only" @("select-window", "-t", "X:1.$P2") 0 "" 1 0
    Test-Select "2 another window's pane by @id.number: window only" @("select-window", "-t", "$($script:W1).$P2") 0 "" 1 0
    Test-Select "3 the current window's pane: nothing moves" @("select-window", "-t", "X:0.$P2") 0 "" 0 1
    Test-Select "4 window 1 and its own %id: window only" @("select-window", "-t", "X:1.$($script:W1Second)") 0 "" 1 0
    Test-Select "5 a bare %id: its window, not its pane" @("select-window", "-t", $script:W1Second) 0 "" 1 0
    Test-Select "6 window 0 with a %id of window 1 is refused" @("select-window", "-t", "X:0.$($script:W1Second)") 1 "can't find pane" 0 1
    Test-Select "7 a missing pane under a symbolic window is refused" @("select-window", "-t", "X:{end}.$Pmiss") 1 "can't find pane" 0 1
    Test-Select "8 a missing window moves no pane" @("select-window", "-t", "X:{swpmissing}.$P2") 1 "can't find window" 0 1
    Test-Select "9 -n with a pane part: next window, no pane" @("select-window", "-n", "-t", "X:0.$P2") 0 "" 1 0
    Test-Select "10 a missing pane by number is refused" @("select-window", "-t", "X:1.$Pmiss") 1 "can't find pane" 0 1
    Test-Select "11 a window alone" @("select-window", "-t", "X:1") 0 "" 1 0
    Test-Select "16 -T with a missing pane is refused" @("select-window", "-T", "-t", "X:{end}.$Pmiss") 1 "can't find pane" 0 1

    Write-Host "`n--- pane-base-index ${Base}: -n, -p and -l check the target first ---" -ForegroundColor Yellow
    Test-Select "17 -n with a missing pane is refused" @("select-window", "-n", "-t", "X:{end}.$Pmiss") 1 "can't find pane" 0 1
    Test-Select "18 -p with a missing pane is refused" @("select-window", "-p", "-t", "X:{end}.$Pmiss") 1 "can't find pane" 0 1
    Test-Select "19 -l with a missing pane is refused" @("select-window", "-l", "-t", "X:{end}.$Pmiss") 1 "can't find pane" 0 1
    Test-Select "20 -n with a %id of another window is refused" @("select-window", "-n", "-t", "X:0.$($script:W1Second)") 1 "can't find pane" 0 1
    Test-Select "21 -n with a pane that exists: next window, no pane" @("select-window", "-n", "-t", "X:{end}.$P2") 0 "" 1 0
    Test-Select "22 -n with a session only: next window" @("select-window", "-n", "-t", "X") 0 "" 1 0


    Write-Host "`n--- pane-base-index ${Base}: X:.P is a select-window of the current window ---" -ForegroundColor Yellow
    # tmux queues the select-window hooks for every select-window
    # (cmd-select-window.c:147). Each hook appends to a user option, and
    # X:.P must leave the same non-empty values as a plain
    # `select-window -t X:1` on the same binary. The values are hook
    # effects, not the number of times a hook ran: an in-process
    # `set-option -ga` can apply more than once.
    $Hooked = @{}
    foreach ($Target in @("X:1", "X:.$P2")) {
        if (-not (Reset-Focus)) { continue }
        Expect-Step @("set-option", "-g", "@swp_before", "") | Out-Null
        Expect-Step @("set-option", "-g", "@swp_after", "") | Out-Null
        Expect-Step @("set-hook", "-g", "before-select-window", "set-option -ga @swp_before x") | Out-Null
        Expect-Step @("set-hook", "-g", "after-select-window", "set-option -ga @swp_after x") | Out-Null
        $Result = Invoke-Psmux @("select-window", "-t", $Target)
        Start-Sleep -Milliseconds 500
        Expect-Step @("set-hook", "-gu", "before-select-window") | Out-Null
        Expect-Step @("set-hook", "-gu", "after-select-window") | Out-Null
        $Hooked[$Target] = @{
            Code = $Result.Code; Err = $Result.Err; State = (Get-State)
            Before = (Expect-Step @("show-options", "-gv", "@swp_before")).Out
            After = (Expect-Step @("show-options", "-gv", "@swp_after")).Out
        }
    }
    $Plain = $Hooked["X:1"]; $Pane = $Hooked["X:.$P2"]
    if ($Plain -and $Pane) {
        $Want = Get-WantState 0 1
        Check ($Pane.Code -eq 0 -and $Pane.State -ceq $Want -and $Plain.Before -ne "" -and $Plain.After -ne "" -and $Pane.Before -eq $Plain.Before -and $Pane.After -eq $Plain.After) "14 X:.$P2 moves nothing and runs the select-window hooks as select-window -t X:1 does" "exit $($Pane.Code), '$($Pane.Err)', before '$($Pane.Before)' after '$($Pane.After)' (X:1: '$($Plain.Before)' '$($Plain.After)'), state '$($Pane.State)' (want '$Want')"
    }
    else { Fail "14 X:.$P2 -> skipped, reset failed" }

    Write-Host "`n--- pane-base-index ${Base}: a control client ---" -ForegroundColor Yellow
    Start-Control
    try {
        Test-ControlSelect "12 another window's pane by number: window only" "select-window -t X:1.$P2" "end" 1 0
        Test-ControlSelect "15 a missing pane is %error" "select-window -t X:1.$Pmiss" "error" 0 1
        Test-ControlSelect "15 window 0 with a %id of window 1 is %error" "select-window -t X:0.$($script:W1Second)" "error" 0 1
        Test-ControlSelect "23 -n with a missing pane is %error" "select-window -n -t X:{end}.$Pmiss" "error" 0 1
    }
    finally { Stop-Control }

    Write-Host "`n--- pane-base-index ${Base}: select-window run by a hook, inside the server ---" -ForegroundColor Yellow
    Test-InProcess "13 a missing pane is not selected" "select-window -t :1.99" 0 1
    Test-InProcess "13 a pane of window 1: window only" "select-window -t :1.$P2" 1 0
    Test-InProcess "13 a bare %id: its window" "select-window -t $($script:W1Second)" 1 0

    # Sessions of one namespace share window and pane ids, so X goes before
    # Y and Z come up; their cases use window indexes only.
    Write-Host "`n--- pane-base-index ${Base}: three windows, so -n, -p and -l land apart ---" -ForegroundColor Yellow
    Expect-Step @("kill-session", "-t", "X") | Out-Null
    Start-Session "Y"
    Expect-Step @("new-window", "-t", "Y") | Out-Null
    Expect-Step @("new-window", "-t", "Y") | Out-Null
    Start-Sleep -Milliseconds 1000
    Expect-Step @("select-window", "-t", "Y:1") | Out-Null
    Expect-Step @("select-window", "-t", "Y:0") | Out-Null
    $script:BaselineY = Get-State "Y"
    if ((Get-WantState 0 1 $script:BaselineY) -cne $script:BaselineY -or ($script:BaselineY -split " \|\| ")[1] -cne "0:1:0 1:0:1 2:0:0") { throw "unexpected start state of Y: '$($script:BaselineY)'" }
    Test-Relative "24 -n checks window 2, then goes next" @("select-window", "-n", "-t", "Y:{end}.$P1") @(, @(1, 0))
    Test-Relative "25 -p checks window 1, then goes previous" @("select-window", "-p", "-t", "Y:1.$P1") @(, @(2, 0))
    Test-Relative "26 -l twice swaps current and last" @("select-window", "-l", "-t", "Y:{end}.$P1") @(@(1, 0), @(0, 1))
    Start-Session "Z"
    Start-Sleep -Milliseconds 500
    $Before = Get-State "Z"
    $Result = Invoke-Psmux @("select-window", "-l", "-t", "Z:0.$P1")
    $After = Get-State "Z"
    Check ($Result.Code -eq 1 -and $Result.Err -match "no last window" -and $After -ceq $Before) "27 -l with a valid target and no last window is refused" "exit $($Result.Code), '$($Result.Err)', state '$After' (was '$Before')"
}

# Session Y has windows 0, 1 and 2, one pane each. Each case starts from
# window 0 current and window 1 last, runs the command once per expected
# step without a reset in between, and compares each step's state.
function Test-Relative([string]$Label, [string[]]$CliArgs, $Steps) {
    Expect-Step @("select-window", "-t", "Y:1") | Out-Null
    Expect-Step @("select-window", "-t", "Y:0") | Out-Null
    $Start = Get-State "Y"
    if ($Start -cne $script:BaselineY) { Fail "$Label -> skipped, reset of Y gave '$Start' (want '$($script:BaselineY)')"; return }
    $Got = @()
    $Ok = $true
    foreach ($Step in $Steps) {
        $Result = Invoke-Psmux $CliArgs
        $State = Get-State "Y"
        $Want = Get-WantState $Step[0] $Step[1] $script:BaselineY
        $Got += "exit $($Result.Code) '$($Result.Err)' '$State' (want '$Want')"
        if ($Result.Code -ne 0 -or $Result.Err -ne "" -or $State -cne $Want) { $Ok = $false }
    }
    Check $Ok "$Label ($($CliArgs -join ' '))" ($Got -join "; ")
}

# A hook runs select-window through the server's own command path (no
# client): after-new-window fires it, and `new-window -d` leaves windows 0
# and 1 as they were apart from what the hook did. The new window 2 is then
# killed, and the next reset checks that it is gone.
function Test-InProcess([string]$Label, [string]$HookCommand, [int]$Current, [int]$Last) {
    if (-not (Reset-Focus)) { Fail "$Label -> skipped, reset failed"; return }
    Expect-Step @("set-hook", "-g", "after-new-window", $HookCommand) | Out-Null
    try {
        Expect-Step @("new-window", "-d", "-t", "X") | Out-Null
        Start-Sleep -Milliseconds 800
        $State = Get-State
    }
    finally {
        Expect-Step @("set-hook", "-gu", "after-new-window") | Out-Null
        $Windows = (Expect-Step @("list-windows", "-t", "X", "-F", "#{window_index}")).Out -split "`r?`n"
        if ($Windows -contains "2") { Expect-Step @("kill-window", "-t", "X:2") | Out-Null }
    }
    $PanePart, $WindowPart = $State -split " \|\| "
    $Old = ($PanePart -split " " | Where-Object { -not $_.StartsWith("2|") }) -join " "
    $New = @($PanePart -split " " | Where-Object { $_.StartsWith("2|") })
    $WantPanes, $WantWindows = (Get-WantState $Current $Last) -split " \|\| "
    $Ok = $Old -ceq $WantPanes -and $WindowPart -ceq "$WantWindows 2:0:0" -and $New.Count -eq 1 -and $New[0] -match '^2\|%\d+\|\d+\|1\|0$'
    Check $Ok "$Label (hook: $HookCommand)" "state '$State' (want '$WantPanes || $WantWindows 2:0:0' and one inactive window 2)"
}

$DefaultBefore = Get-DefaultLs
$SetupOk = $true
$ServerPids = @()
foreach ($Base in @(0, 1)) {
    $script:Ns = "swp" + $Tag + "b" + $Base
    $script:DataDir = Join-Path $Root "data$Base"
    $HomeDir = Join-Path $Root "home$Base"
    $ConfigFile = Join-Path $Root "base$Base.conf"
    $RunOk = $false
    try {
        New-Item -ItemType Directory -Force $script:DataDir, $HomeDir | Out-Null
        [System.IO.File]::WriteAllText($ConfigFile, "set -g pane-base-index $Base`n")
        [System.IO.File]::WriteAllText((Join-Path $HomeDir ".psmux.conf"), "")
        [System.IO.File]::WriteAllText((Join-Path $HomeDir ".tmux.conf"), "")
        foreach ($Name in $TouchedNames) { Remove-Item "Env:$Name" -ErrorAction SilentlyContinue }
        $env:PSMUX_DATA_DIR = $script:DataDir
        $env:PSMUX_CONFIG_FILE = $ConfigFile
        $env:USERPROFILE = $HomeDir
        $env:HOME = $HomeDir

        Write-Host "`n=== select-window with a pane part, pane-base-index $Base ===" -ForegroundColor Cyan
        Write-Host "  Binary:    $Psmux"
        Write-Host "  Namespace: $($script:Ns)"

        Start-Session "X"
        Expect-Step @("split-window", "-t", "X:0") | Out-Null
        Expect-Step @("new-window", "-t", "X") | Out-Null
        Expect-Step @("split-window", "-t", "X:1") | Out-Null
        Start-Sleep -Milliseconds 1500
        $Rows = @((Expect-Step @("list-panes", "-s", "-t", "X", "-F", "#{window_index}|#{window_id}|#{pane_index}|#{pane_id}")).Out -split "`r?`n")
        $Layout = ($Rows | ForEach-Object { $Field = $_.Split("|"); "$($Field[0]).$($Field[2])" }) -join ","
        if ($Layout -ne "0.$Base,0.$($Base + 1),1.$Base,1.$($Base + 1)") { throw "unexpected layout '$Layout'" }
        $script:W0 = $Rows[0].Split("|")[1]; $script:W0First = $Rows[0].Split("|")[3]
        $script:W1 = $Rows[2].Split("|")[1]; $script:W1First = $Rows[2].Split("|")[3]; $script:W1Second = $Rows[3].Split("|")[3]

        Expect-Step @("select-window", "-t", $script:W1) | Out-Null
        Expect-Step @("select-pane", "-t", $script:W1First) | Out-Null
        Expect-Step @("select-window", "-t", $script:W0) | Out-Null
        Expect-Step @("select-pane", "-t", $script:W0First) | Out-Null
        $script:Baseline = Get-State
        if ((Get-WantState 0 1) -cne $script:Baseline) { throw "start state is not window 0 current, window 1 last: '$($script:Baseline)'" }
        Write-Host "  Start:     $($script:Baseline)"

        Invoke-Suite $Base
        $RunOk = $true
    }
    catch {
        Fail "setup or unexpected error: $_"
    }
    finally {
        Stop-Control
        $Kill = & $Psmux -L $script:Ns kill-server 2>&1
        $KillCode = $LASTEXITCODE
        $Alive = @()
        $Deadline = (Get-Date).AddSeconds(10)
        do {
            $Alive = @($ServerPids | Where-Object { $Proc = Get-Process -Id $_ -ErrorAction SilentlyContinue; $Proc -and $Proc.Path -eq $Psmux })
            if ($Alive.Count -eq 0) { break }
            Start-Sleep -Milliseconds 200
        } while ((Get-Date) -lt $Deadline)
        Restore-Env
        if ($ServerPids.Count -gt 0 -and $Alive.Count -ne 0) { Fail "cleanup: server processes still running: $($Alive -join ', ')" }
        elseif ($ServerPids.Count -gt 0 -and $KillCode -ne 0) { Fail "cleanup: kill-server exited $KillCode ($Kill)" }
        else { Pass "cleanup: namespace $($script:Ns) has no server left" }
        if (-not $RunOk) { $SetupOk = $false }
    }
}

$DefaultAfter = Get-DefaultLs
if ($DefaultAfter -ceq $DefaultBefore) { Pass "default namespace ls unchanged" }
else { Fail "default namespace ls changed:`n$DefaultBefore`n---`n$DefaultAfter" }
Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0 -or -not $SetupOk) { exit 1 }
exit 0
