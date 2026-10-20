# CLI -t validation of a numeric pane index after a window id (`@N.P`) and
# after an empty window part (`X:.P`).
#
# The #545 client side check looked a numeric pane index up only when a window
# was named by index or name. After a window id it checked only `%id` panes,
# and with an empty window part it checked nothing, so `-t @N.99` and `-t X:.99`
# reached the server, whose "ERROR: can't find pane: 99" reply was printed on
# stdout at exit 0 by list-panes, list-windows, list-clients, choose-buffer,
# choose-client and find-window. Now the index is looked up in that window
# (`@N.P`) or in the session's active window (`X:.P`).
#
# Controls: the active window has pane 0 only and the other window has panes 0
# and 1. `@<other>.1` works, `X:.1` is refused while the one-pane window is
# active and works once the two-pane window is active.
#
# Isolation: a unique -L namespace, a PSMUX_DATA_DIR, USERPROFILE and HOME of
# its own, an empty config, and cleanup only with `-L <ns> kill-server`. The
# default namespace's `ls` is compared before and after. Any mismatch, timeout,
# setup failure or cleanup failure exits non-zero.
#
# PSMUX_TEST_EXE picks the binary (default: psmux on PATH). PSMUX_TEST_ROOT
# picks where the scratch folder goes (default: the system temp folder).

$ErrorActionPreference = "Stop"
$script:Passed = 0
$script:Failed = 0
function Pass($Message) { Write-Host "  [PASS] $Message" -ForegroundColor Green; $script:Passed++ }
function Fail($Message) { Write-Host "  [FAIL] $Message" -ForegroundColor Red; $script:Failed++ }

$Psmux = $env:PSMUX_TEST_EXE
if (-not $Psmux) { $Found = Get-Command psmux -ErrorAction SilentlyContinue; if ($Found) { $Psmux = $Found.Source } }
if (-not $Psmux) { Write-Host "psmux binary not found"; exit 2 }
$Psmux = (Resolve-Path $Psmux).Path

$RootBase = if ($env:PSMUX_TEST_ROOT) { $env:PSMUX_TEST_ROOT } else { [System.IO.Path]::GetTempPath() }
$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$Root = Join-Path $RootBase ("psmux_tvp_" + $Tag)
$Ns = "tvp" + $Tag
$DataDir = Join-Path $Root "data"
$HomeDir = Join-Path $Root "home"
$ConfigFile = Join-Path $Root "empty.conf"

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
    foreach ($Arg in @("-L", $Ns) + $CliArgs) { $Info.ArgumentList.Add($Arg) }
    $Info.UseShellExecute = $false
    $Info.RedirectStandardOutput = $true
    $Info.RedirectStandardError = $true
    $Process = [System.Diagnostics.Process]::Start($Info)
    $StdoutTask = $Process.StandardOutput.ReadToEndAsync()
    $StderrTask = $Process.StandardError.ReadToEndAsync()
    if (-not $Process.WaitForExit($TimeoutMs)) {
        $Process.Kill()
        $Process.WaitForExit()
        return @{ Code = $null; Out = ""; Err = ""; TimedOut = $true }
    }
    $Process.WaitForExit()
    return @{ Code = $Process.ExitCode; Out = $StdoutTask.Result; Err = $StderrTask.Result; TimedOut = $false }
}

function With-Target([string[]]$Command, [string]$Target) {
    $Rest = if ($Command.Count -gt 1) { $Command[1..($Command.Count - 1)] } else { @() }
    return @($Command[0], "-t", $Target) + $Rest
}

function Expect-PaneRefused([string[]]$CliArgs) {
    $Label = $CliArgs -join " "
    $Result = Invoke-Psmux $CliArgs
    if ($Result.TimedOut) { Fail "$Label timed out"; return }
    if ($Result.Code -eq 1 -and $Result.Out -eq "" -and $Result.Err -match "can't find pane") {
        Pass "$Label -> exit 1, stderr '$($Result.Err.Trim())'"
    } else {
        Fail "$Label -> exit $($Result.Code), stdout '$($Result.Out.Trim())', stderr '$($Result.Err.Trim())'"
    }
}

function Expect-Ok([string[]]$CliArgs) {
    $Label = $CliArgs -join " "
    $Result = Invoke-Psmux $CliArgs
    if ($Result.TimedOut) { Fail "$Label timed out"; return }
    if ($Result.Code -eq 0 -and $Result.Out -notmatch "(?m)^ERROR") {
        Pass "$Label -> exit 0"
    } else {
        Fail "$Label -> exit $($Result.Code), stdout '$($Result.Out.Trim())', stderr '$($Result.Err.Trim())'"
    }
}

function Expect-Step([string[]]$CliArgs) {
    $Result = Invoke-Psmux $CliArgs
    if ($Result.TimedOut -or $Result.Code -ne 0) {
        throw "setup step '$($CliArgs -join ' ')' failed: exit $($Result.Code), $($Result.Err.Trim())"
    }
    return $Result
}

$DefaultBefore = Get-DefaultLs
$SetupOk = $false
$ServerPids = @()
try {
    New-Item -ItemType Directory -Force $DataDir, $HomeDir | Out-Null
    foreach ($File in @($ConfigFile, (Join-Path $HomeDir ".psmux.conf"), (Join-Path $HomeDir ".tmux.conf"))) {
        [System.IO.File]::WriteAllText($File, "")
    }
    foreach ($Name in $TouchedNames) { Remove-Item "Env:$Name" -ErrorAction SilentlyContinue }
    $env:PSMUX_DATA_DIR = $DataDir
    $env:PSMUX_CONFIG_FILE = $ConfigFile
    $env:USERPROFILE = $HomeDir
    $env:HOME = $HomeDir

    Write-Host "`n=== CLI -t validation: numeric pane index after @id and after an empty window ===" -ForegroundColor Cyan
    Write-Host "  Binary:    $Psmux"
    Write-Host "  Namespace: $Ns"

    & $Psmux -L $Ns new-session -d -s X -n tvpone *> $null
    $Deadline = (Get-Date).AddSeconds(15)
    do {
        $Probe = Invoke-Psmux @("has-session", "-t", "X") 5000
        if ($Probe.Code -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $Deadline)
    foreach ($PidFile in Get-ChildItem $DataDir -Filter "*.pid" -ErrorAction SilentlyContinue) {
        $ServerPids += [int](([System.IO.File]::ReadAllText($PidFile.FullName)).Trim().Split(":")[0])
    }
    if ($Probe.Code -ne 0) { throw "session X did not come up" }

    Expect-Step @("new-window", "-t", "X", "-n", "tvptwo") | Out-Null
    $Windows = (Expect-Step @("list-windows", "-t", "X", "-F", "#{window_name}|#{window_id}")).Out
    $OneId = ($Windows -split "`r?`n" | Where-Object { $_ -like "tvpone|*" }) -replace '^tvpone\|', ''
    $TwoId = ($Windows -split "`r?`n" | Where-Object { $_ -like "tvptwo|*" }) -replace '^tvptwo\|', ''
    if ($OneId -notmatch '^@\d+$' -or $TwoId -notmatch '^@\d+$') { throw "window ids not found in '$Windows'" }
    Expect-Step @("split-window", "-t", "${TwoId}") | Out-Null
    Expect-Step @("select-window", "-t", "${OneId}") | Out-Null
    $Layout = (Expect-Step @("list-panes", "-a", "-F", "#{window_id}|#{pane_index}|#{window_active}")).Out.Trim() -split "`r?`n" | Sort-Object
    $Want = @("${OneId}|0|1", "${TwoId}|0|0", "${TwoId}|1|0") | Sort-Object
    if (($Layout -join ",") -ne ($Want -join ",")) { throw "unexpected layout '$($Layout -join ',')', want '$($Want -join ',')'" }
    $SetupOk = $true
    Write-Host "  Windows:   $OneId (active, pane 0), $TwoId (panes 0 and 1)"

    $Commands = @(
        @("list-panes"), @("list-windows"), @("list-clients"),
        @("choose-buffer"), @("choose-client"), @("find-window", "tvp")
    )

    Write-Host "`n--- a pane index that does not exist is refused by the CLI ---" -ForegroundColor Yellow
    foreach ($Command in $Commands) {
        Expect-PaneRefused (With-Target $Command "$OneId.99")
        Expect-PaneRefused (With-Target $Command "X:.99")
    }

    Write-Host "`n--- @id.P looks in that window, X:.P in the active one ---" -ForegroundColor Yellow
    foreach ($Command in $Commands) {
        Expect-Ok (With-Target $Command "$TwoId.1")
        Expect-PaneRefused (With-Target $Command "X:.1")
    }

    Expect-Step @("select-window", "-t", "${TwoId}") | Out-Null
    Write-Host "`n--- with the two-pane window active, X:.1 exists ---" -ForegroundColor Yellow
    foreach ($Command in $Commands) {
        Expect-Ok (With-Target $Command "X:.1")
    }
}
catch {
    Fail "setup or unexpected error: $_"
}
finally {
    $Kill = & $Psmux -L $Ns kill-server 2>&1
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
    else { Pass "cleanup: namespace $Ns has no server left" }
    $DefaultAfter = Get-DefaultLs
    if ($DefaultAfter -ceq $DefaultBefore) { Pass "default namespace ls unchanged" }
    else { Fail "default namespace ls changed:`n$DefaultBefore`n---`n$DefaultAfter" }
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0 -or -not $SetupOk) { exit 1 }
exit 0
