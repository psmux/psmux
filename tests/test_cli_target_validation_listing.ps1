# CLI -t validation for list-windows, list-clients, choose-buffer,
# choose-client and find-window.
#
# These commands print the server's reply as their output. Without the #545
# client side check, a -t naming a window or pane that does not exist reached
# the server, whose "ERROR: can't find ..." reply was printed on stdout and the
# command exited 0. With the check they exit 1 with nothing on stdout.
#
# Controls: a valid -t still works, and `list-windows -F 'ERROR: m40'` still
# prints that text at exit 0 (ordinary output that starts with ERROR is not an
# error).
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
$Root = Join-Path $RootBase ("psmux_tvl_" + $Tag)
$Ns = "tvl" + $Tag
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

function Expect-Refused([string[]]$CliArgs) {
    $Label = $CliArgs -join " "
    $Result = Invoke-Psmux $CliArgs
    if ($Result.TimedOut) { Fail "$Label timed out"; return }
    if ($Result.Code -eq 1 -and $Result.Out -eq "" -and $Result.Err.Trim() -ne "") {
        Pass "$Label -> exit 1, stderr '$($Result.Err.Trim())'"
    } else {
        Fail "$Label -> exit $($Result.Code), stdout '$($Result.Out.Trim())', stderr '$($Result.Err.Trim())'"
    }
}

function Expect-Ok([string[]]$CliArgs, $ExpectedOut = $null) {
    $Label = $CliArgs -join " "
    $Result = Invoke-Psmux $CliArgs
    if ($Result.TimedOut) { Fail "$Label timed out"; return }
    $OutOk = if ($null -eq $ExpectedOut) { $Result.Out -notmatch "(?m)^ERROR" } else { $Result.Out.Trim() -eq $ExpectedOut }
    if ($Result.Code -eq 0 -and $OutOk) {
        Pass "$Label -> exit 0"
    } else {
        Fail "$Label -> exit $($Result.Code), stdout '$($Result.Out.Trim())', stderr '$($Result.Err.Trim())'"
    }
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

    Write-Host "`n=== CLI -t validation: list-windows, list-clients, choose-buffer, choose-client, find-window ===" -ForegroundColor Cyan
    Write-Host "  Binary:    $Psmux"
    Write-Host "  Namespace: $Ns"

    & $Psmux -L $Ns new-session -d -s X -n tvlwin *> $null
    $Deadline = (Get-Date).AddSeconds(15)
    do {
        $Probe = Invoke-Psmux @("has-session", "-t", "X") 5000
        if ($Probe.Code -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $Deadline)
    foreach ($PidFile in Get-ChildItem $DataDir -Filter "*.pid" -ErrorAction SilentlyContinue) {
        $ServerPids += [int](([System.IO.File]::ReadAllText($PidFile.FullName)).Trim().Split(":")[0])
    }
    $Panes = Invoke-Psmux @("list-panes", "-t", "X", "-F", "#{pane_id}")
    $PaneId = $Panes.Out.Trim()
    if ($Probe.Code -ne 0 -or $Panes.Code -ne 0 -or $PaneId -notmatch '^%\d+$') {
        Fail "setup: session X did not come up (has-session $($Probe.Code), pane '$PaneId')"
    } else {
        $SetupOk = $true
        $MissingPaneId = "%" + ([int]$PaneId.Substring(1) + 1000)
        Write-Host "  Pane:      $PaneId (missing: $MissingPaneId)"

        $Commands = @(
            @("list-windows"), @("lsw"),
            @("list-clients"), @("lsc"),
            @("choose-buffer"), @("chooseb"),
            @("choose-client"),
            @("find-window", "tvlwin"), @("findw", "tvlwin")
        )

        Write-Host "`n--- a target that does not exist is refused by the CLI ---" -ForegroundColor Yellow
        foreach ($Command in $Commands) {
            foreach ($Target in @("X:99", "X:0.99", $MissingPaneId)) {
                Expect-Refused (With-Target $Command $Target)
            }
        }

        Write-Host "`n--- a target that exists still works ---" -ForegroundColor Yellow
        foreach ($Command in $Commands) {
            foreach ($Target in @("X", "X:0", "X:0.0", $PaneId)) {
                Expect-Ok (With-Target $Command $Target)
            }
        }

        Write-Host "`n--- output that starts with ERROR is still output ---" -ForegroundColor Yellow
        Expect-Ok @("list-windows", "-t", "X", "-F", "ERROR: m40") "ERROR: m40"
    }
}
catch {
    Fail "unexpected error: $_"
}
finally {
    $Kill = & $Psmux -L $Ns kill-server 2>&1
    $KillCode = $LASTEXITCODE
    $Deadline = (Get-Date).AddSeconds(10)
    do {
        $Alive = @($ServerPids | Where-Object { $Proc = Get-Process -Id $_ -ErrorAction SilentlyContinue; $Proc -and $Proc.Path -eq $Psmux })
        if ($Alive.Count -eq 0) { break }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $Deadline)
    Restore-Env
    if ($ServerPids.Count -gt 0 -and $Alive.Count -ne 0) { Fail "cleanup: server processes still running: $($Alive -join ', ')" }
    elseif ($SetupOk -and $KillCode -ne 0) { Fail "cleanup: kill-server exited $KillCode ($Kill)" }
    else { Pass "cleanup: namespace $Ns has no server left" }
    $DefaultAfter = Get-DefaultLs
    if ($DefaultAfter -ceq $DefaultBefore) { Pass "default namespace ls unchanged" }
    else { Fail "default namespace ls changed:`n$DefaultBefore`n---`n$DefaultAfter" }
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0 -or -not $SetupOk) { exit 1 }
exit 0
