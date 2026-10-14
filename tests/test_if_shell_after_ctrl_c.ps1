param(
    [Parameter(Mandatory)][string]$PsmuxBinary
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This regression requires Windows.' }
$PsmuxBinary = (Resolve-Path -LiteralPath $PsmuxBinary).Path
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('psmux-if-shell-stdin-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $testRoot
$dataDirectory = Join-Path $testRoot 'data'
$null = New-Item -ItemType Directory -Path $dataDirectory
$socket = 'psmux-if-shell-stdin-' + [guid]::NewGuid().ToString('N')
$config = Join-Path $testRoot 'psmux.conf'
[IO.File]::WriteAllText($config, "set -g default-shell cmd.exe`nset -g remain-on-exit on`n")
$records = [Collections.Generic.List[object]]::new()
$ownedProcesses = [Collections.Generic.List[object]]::new()
$sessionStarted = $false

function Invoke-TestPsmux([string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new($PsmuxBinary)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['PSMUX_DATA_DIR'] = $dataDirectory
    foreach ($key in @($start.Environment.Keys)) {
        if ($key -eq 'TMUX' -or $key -eq 'TMUX_PANE' -or ($key.StartsWith('PSMUX_') -and $key -ne 'PSMUX_DATA_DIR')) {
            $null = $start.Environment.Remove($key)
        }
    }
    foreach ($argument in @('-L', $socket, '-f', $config) + $Arguments) { $start.ArgumentList.Add($argument) }
    $child = [Diagnostics.Process]::Start($start)
    try {
        # The launcher has pipe stdin that closes, then the launcher exits.
        $child.StandardInput.Close()
        $stdout = $child.StandardOutput.ReadToEndAsync()
        $stderr = $child.StandardError.ReadToEndAsync()
        if (-not $child.WaitForExit(20000)) { $child.Kill(); throw 'Private psmux command timed out.' }
        $result = [pscustomobject]@{ command = $Arguments; exitCode = $child.ExitCode; stdout = $stdout.Result; stderr = $stderr.Result }
        $records.Add($result)
        if ($result.exitCode -ne 0) { throw "Private psmux command failed: $($result.stderr)" }
        return $result.stdout.Trim()
    } finally { $child.Dispose() }
}

try {
    $null = Invoke-TestPsmux @('new-session', '-d', '-s', 'stdin', '--', 'cmd.exe', '/d', '/k')
    $sessionStarted = $true
    foreach ($format in @('#{pid}', '#{pane_pid}')) {
        $processId = [int](Invoke-TestPsmux @('display-message', '-p', $format))
        $process = Get-Process -Id $processId
        $ownedProcesses.Add([pscustomobject]@{ pid = $processId; startTicks = $process.StartTime.ToUniversalTime().Ticks })
    }
    $serverPid = $ownedProcesses[0].pid
    # #{pid} forces the CLI to forward the condition to the server, where
    # format expansion and condition execution must both take place.
    $condition = "if ([int]'#{pid}' -eq $serverPid) { exit 0 } exit 1"
    if ((Invoke-TestPsmux @('if-shell', $condition, 'display-message -p ACCEPTED', 'display-message -p REJECTED')) -ne 'ACCEPTED') {
        throw 'Initial forwarded condition rejected the current server.'
    }
    $null = Invoke-TestPsmux @('send-keys', '-t', 'stdin:0.0', '-l', 'echo benign')
    $null = Invoke-TestPsmux @('send-keys', '-t', 'stdin:0.0', 'Enter')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $pane = Invoke-TestPsmux @('capture-pane', '-p', '-t', 'stdin:0.0')
        $ready = ($pane -split "`n" | Where-Object { $_.Trim() -eq 'benign' }).Count -gt 0
        if (-not $ready) { Start-Sleep -Milliseconds 50 }
    } while (-not $ready -and [DateTime]::UtcNow -lt $deadline)
    if (-not $ready) { throw 'The benign pane command did not execute.' }
    $null = Invoke-TestPsmux @('send-keys', '-t', 'stdin:0.0', 'C-c')
    # A run-shell request also establishes that the input dispatch has run.
    if ((Invoke-TestPsmux @('run-shell', "Write-Output '#{pid}'")) -ne "$serverPid") { throw 'Server format expansion changed.' }
    if ((Invoke-TestPsmux @('if-shell', $condition, 'display-message -p ACCEPTED', 'display-message -p REJECTED')) -ne 'ACCEPTED') {
        throw 'Forwarded if-shell could not launch its condition after Ctrl+C.'
    }
    if ((Invoke-TestPsmux @('if-shell', "exit 1 # #{pid}", 'display-message -p ACCEPTED', 'display-message -p REJECTED')) -ne 'REJECTED') {
        throw 'A failed condition must still select the false branch.'
    }
    Write-Output 'PASS: forwarded if-shell survives Ctrl+C console detachment and retains its exit-code verdict.'
} finally {
    if ($sessionStarted) { $null = Invoke-TestPsmux @('kill-server') }
    foreach ($owned in $ownedProcesses) {
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            $current = Get-Process -Id $owned.pid -ErrorAction SilentlyContinue
            $same = $null -ne $current -and $current.StartTime.ToUniversalTime().Ticks -eq $owned.startTicks
            if ($same) { Start-Sleep -Milliseconds 50 }
        } while ($same -and [DateTime]::UtcNow -lt $deadline)
        if ($same) { throw "Owned process $($owned.pid) remained alive after private cleanup." }
    }
    [IO.File]::WriteAllText((Join-Path $testRoot 'results.json'), ($records | ConvertTo-Json -Depth 5))
    [IO.File]::WriteAllText((Join-Path $testRoot 'process-identities.json'), ($ownedProcesses | ConvertTo-Json))
    Write-Output "Artifacts retained: $testRoot"
}
