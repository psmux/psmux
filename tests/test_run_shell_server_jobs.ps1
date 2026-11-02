# run-shell ownership, launch acknowledgement, output/status, env and cwd.
[CmdletBinding()]
param(
    [string]$PsmuxExe = $env:PSMUX_EXE
)
$ErrorActionPreference = 'Stop'
if (-not $PsmuxExe) {
    throw 'Set PSMUX_EXE or pass -PsmuxExe to select the binary under test.'
}
$Binary = (Resolve-Path $PsmuxExe).Path
$null = Get-Command pwsh -ErrorAction Stop
$ns = 'rsjobs_' + [guid]::NewGuid().ToString('N')
$root = Join-Path $env:TEMP $ns
$session = 'jobs'
$saved = @{}
foreach ($name in 'PSMUX_DATA_DIR','PSMUX_NO_WARM','PSMUX_TARGET_SESSION','PSMUX_TARGET_FULL','PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_JOB_PROBE','PSMUX_CONFIG_FILE','PSMUX_SOCKET_PATH','USERPROFILE','HOME') {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name)
}
function Assert($condition, $message) {
    if (-not $condition) { throw $message }
    Write-Host "PASS: $message"
}
function Default-Sessions {
    # Read only. Never stop the default namespace.
    $result = & $Binary -L default list-sessions 2>&1
    $code = $LASTEXITCODE
    return "$code`n$($result -join "`n")"
}
function P { & $Binary -L $ns @args 2>&1 }
function Wait-Until([scriptblock]$condition) {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        if (& $condition) { return $true }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    return $false
}
function Connect([string]$mode) {
    $port = [int](Get-Content (Join-Path $env:PSMUX_DATA_DIR "${ns}__${session}.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "${ns}__${session}.key") -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $tcp.ReceiveTimeout = 5000
    $writer = [IO.StreamWriter]::new($tcp.GetStream())
    $writer.AutoFlush = $true
    $reader = [IO.StreamReader]::new($tcp.GetStream())
    $writer.WriteLine("AUTH $key")
    Assert ($reader.ReadLine() -eq 'OK') 'TCP authentication'
    $writer.WriteLine($mode)
    return @{ Tcp = $tcp; Writer = $writer; Reader = $reader }
}
function Server-State {
    $c = Connect 'PERSISTENT'
    try {
        $c.Writer.WriteLine('dump-state')
        for ($i = 0; $i -lt 80; $i++) {
            $line = $c.Reader.ReadLine()
            if ($null -eq $line) { throw 'EOF before dump-state' }
            if ($line.StartsWith('{')) {
                return ($line | ConvertFrom-Json)
            }
        }
        throw 'No dump-state frame'
    } finally { $c.Tcp.Dispose() }
}
function Popup-Text { return ((Server-State).popup_lines -join "`n") }
# A -b job (owner decision on #966): only a failure, on the status line, and
# never the modal popup, whose output would take the client's keys.
function Bg-Failure-Shown([string]$token, [string]$output) {
    $st = Server-State
    $status = "$($st.status_message)"
    $popup = ($st.popup_lines -join "`n")
    return ($status.Contains($token) -and $status.Contains('returned 7') -and -not $popup.Contains($output))
}
function Run-Bytes([string]$command, [string[]]$flags = @()) {
    $info = [Diagnostics.ProcessStartInfo]::new($Binary)
    $info.WorkingDirectory = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($arg in @('-L', $ns, 'run-shell') + $flags + @($command)) { $info.ArgumentList.Add($arg) }
    $client = [Diagnostics.Process]::Start($info)
    $stdout = [IO.MemoryStream]::new()
    $stderr = [IO.MemoryStream]::new()
    try {
        $outTask = $client.StandardOutput.BaseStream.CopyToAsync($stdout)
        $errTask = $client.StandardError.BaseStream.CopyToAsync($stderr)
        Assert ($client.WaitForExit(15000)) 'native CLI finishes'
        $null = $outTask.GetAwaiter().GetResult()
        $null = $errTask.GetAwaiter().GetResult()
        return @{ Code = $client.ExitCode; Pid = $client.Id; Out = $stdout.ToArray(); Err = $stderr.ToArray() }
    } finally {
        if (-not $client.HasExited) { $client.Kill() }
        $client.Dispose(); $stdout.Dispose(); $stderr.Dispose()
    }
}
function Launch([string]$command, [string[]]$flags) {
    $info = [Diagnostics.ProcessStartInfo]::new($Binary)
    $info.WorkingDirectory = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($arg in @('-L', $ns, 'run-shell') + $flags + @($command)) { $info.ArgumentList.Add($arg) }
    $client = [Diagnostics.Process]::Start($info)
    try {
        Assert ($client.WaitForExit(6000)) 'background client returns before job gate opens'
        Assert ($client.ExitCode -eq 0) 'background returns launch success, not exit 7'
        Assert (Wait-Until { Test-Path $report }) 'job has started while client has exited and gate is closed'
    } finally {
        # Unblock only this suite's child even if an assertion fails.
        New-Item -ItemType File -Force $gate | Out-Null
        if (-not $client.HasExited) { $null = $client.WaitForExit(6000) }
        if (-not $client.HasExited) { $client.Kill() }
    }
    try {
        $stdout = $client.StandardOutput.ReadToEnd()
        $stderr = $client.StandardError.ReadToEnd()
        Assert ($stdout -eq '' -and $stderr -eq '') 'native background reply contains launch only'
    } finally { $client.Dispose() }
}
$before = Default-Sessions
$originalCwd = Get-Location
$primaryFailure = $null
try {
    New-Item -ItemType Directory $root | Out-Null
    $serverCwd = New-Item -ItemType Directory (Join-Path $root 'server cwd')
    $callerCwd = New-Item -ItemType Directory (Join-Path $root 'caller cwd ü')
    $explicitCwd = New-Item -ItemType Directory (Join-Path $serverCwd.FullName 'explicit cwd')
    $jobHome = New-Item -ItemType Directory (Join-Path $root 'job home')
    $env:USERPROFILE = $jobHome.FullName
    $env:HOME = $jobHome.FullName
    $env:PSMUX_DATA_DIR = Join-Path $root 'data'
    New-Item -ItemType Directory $env:PSMUX_DATA_DIR | Out-Null
    $env:PSMUX_NO_WARM = '1'
    foreach ($name in 'PSMUX_TARGET_SESSION','PSMUX_TARGET_FULL','PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_SOCKET_PATH') { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
    $env:PSMUX_JOB_PROBE = 'server'
    $conf = Join-Path $root 'empty.conf'
    Set-Content $conf ''
    $env:PSMUX_CONFIG_FILE = $conf
    $env:PSMUX_TARGET_SESSION = "${ns}__${session}"
    $portFile = Join-Path $env:PSMUX_DATA_DIR "${ns}__${session}.port"
    $keyFile = Join-Path $env:PSMUX_DATA_DIR "${ns}__${session}.key"
    $pidFile = Join-Path $env:PSMUX_DATA_DIR "${ns}__${session}.pid"
    $marker = Join-Path $root 'reject-marker.ps1'
    @'
param($Report)
[IO.File]::WriteAllText($Report, 'LOCAL_EXECUTION')
exit 7
'@ | Set-Content $marker
    foreach ($background in $false,$true) {
        $flags = @(); if ($background) { $flags = @('-b') }
        $report = Join-Path $root "missing-server-$background.txt"
        Assert (-not (Test-Path $portFile) -and -not (Test-Path $report)) 'missing-server port and report start absent'
        $result = Run-Bytes "pwsh -NoProfile -File '$marker' '$report'" $flags
        # No server: the CLI runs the command itself, as psmux did before the
        # native reply (owner decision on #966).
        Assert (-not (Test-Path $portFile)) 'missing server creates no endpoint'
        Assert (Wait-Until { Test-Path $report }) "missing server runs the command in the CLI (background=$background)"
        $expected = if ($background) { 0 } else { 7 }
        Assert ($result.Code -eq $expected) "missing-server run-shell keeps the pre-native exit code $expected (background=$background, got $($result.Code))"
        Remove-Item $report
    }
    $nativeAck = "$([char]1)PSMUX_RUN_SHELL_NATIVE_V1$([char]1)`n"
    foreach ($case in @(
        # Legacy: the peer acts as a server from an older psmux (it takes a plain
        # AUTH), so the CLI falls back to the pre-native run-shell.
        @{ Name = 'ordinary-ack'; Ack = "OK`n"; Reply = ''; Submitted = $false; Legacy = $true },
        @{ Name = 'auth-refusal'; Ack = "ERROR: Invalid session key`n"; Reply = ''; Submitted = $false; Legacy = $true },
        @{ Name = 'invalid-ack'; Ack = "unexpected reply`n"; Reply = ''; Submitted = $false; Legacy = $false },
        @{ Name = 'invalid-frame'; Ack = $nativeAck; Reply = "RS1 bad 0 0`n"; Submitted = $true; Legacy = $false },
        @{ Name = 'truncated-frame'; Ack = $nativeAck; Reply = "RS1 7 5 0`nhi"; Submitted = $true; Legacy = $false }
    )) {
        foreach ($background in $false,$true) {
            $flags = @(); if ($background) { $flags = @('-b') }
            $report = Join-Path $root "$($case.Name)-$background.txt"
            Assert (-not (Test-Path $portFile) -and -not (Test-Path $report)) 'controlled-peer endpoint and report start absent'
            $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
            $connections = [Collections.Concurrent.ConcurrentQueue[object]]::new()
            $done = [Threading.ManualResetEventSlim]::new($false)
            $worker = [PowerShell]::Create()
            try {
                $listener.Start()
                $key = [guid]::NewGuid().ToString('N')
                # This process owns the peer; its live registry anchor prevents an unrelated startup AUTH probe.
                $owner = [Diagnostics.Process]::GetCurrentProcess()
                try { Set-Content $pidFile "${PID}:$($owner.StartTime.ToFileTimeUtc())" } finally { $owner.Dispose() }
                Set-Content $keyFile $key
                Set-Content $portFile $listener.LocalEndpoint.Port
                $null = $worker.AddScript({
                    param($listener, $connections, $done, $key, $ack, $reply)
                    $ErrorActionPreference = 'Stop'
                    $deadline = [DateTime]::UtcNow.AddSeconds(30)
                    while (-not $done.IsSet) {
                        if ([DateTime]::UtcNow -gt $deadline) { throw 'Controlled peer exceeded its deadline' }
                        if (-not $listener.Pending()) { $null = $done.Wait(10); continue }
                        $tcp = $listener.AcceptTcpClient()
                        $record = [pscustomobject]@{ Auth = $null; Body = $null }
                        $connections.Enqueue($record)
                        try {
                            $tcp.ReceiveTimeout = 3000; $tcp.SendTimeout = 3000
                            $reader = [IO.StreamReader]::new($tcp.GetStream())
                            $writer = [IO.StreamWriter]::new($tcp.GetStream())
                            $writer.AutoFlush = $true
                            $record.Auth = $reader.ReadLine()
                            # Accept AUTH-only reconnects too, so fallback attempts cannot hide behind a refused second connection.
                            $answer = if ($record.Auth -eq "AUTH $key") { "OK`n" } else { $ack }
                            $writer.Write($answer)
                            $record.Body = $reader.ReadToEnd()
                            if ($record.Auth -ne "AUTH $key") { $writer.Write($reply) }
                        } finally { $tcp.Dispose() }
                    }
                }.ToString()).AddArgument($listener).AddArgument($connections).AddArgument($done).AddArgument($key).AddArgument($case.Ack).AddArgument($case.Reply)
                $pending = $worker.BeginInvoke()
                try {
                    $result = Run-Bytes "pwsh -NoProfile -File '$marker' '$report'" $flags
                } finally {
                    $done.Set()
                    $null = $worker.EndInvoke($pending)
                }
                $observed = @($connections.ToArray())
                $trace = ConvertTo-Json -Compress -Depth 4 -InputObject @{ Connections = $observed; Code = $result.Code; ReportExists = (Test-Path $report); Err = [Text.Encoding]::UTF8.GetString([byte[]]$result.Err) }
                Set-Content (Join-Path $root "$($case.Name)-$background-peer.json") $trace
                Assert ($worker.Streams.Error.Count -eq 0) "controlled peer completes without errors: $($worker.Streams.Error)"
                Assert ($observed[0].Auth -ceq "AUTH $key PSMUX_RUN_SHELL_NATIVE_V1") 'run-shell requires native AUTH acknowledgement'
                if ($case.Legacy) {
                    # Nothing went over the native connection, and no later one
                    # asks for the native reply again: the command runs once.
                    Assert ($observed[0].Body -ceq '') "older server: nothing submitted on the native connection ($trace)"
                    Assert (@($observed | Select-Object -Skip 1 | Where-Object { $_.Auth -cne "AUTH $key" }).Count -eq 0) "older server: later connections are plain AUTH only ($trace)"
                    if ($background) {
                        # Pre-native -b goes to the server as the legacy line.
                        $legacy = @($observed | Where-Object { $_.Body -and $_.Body.Contains('run-shell -b ') })
                        Assert ($legacy.Count -eq 1 -and -not $legacy[0].Body.Contains(' -- ')) "older server: -b goes over once as the legacy line ($trace)"
                        Assert (-not (Test-Path $report) -and $result.Code -eq 0) "older server: -b is the server's, not run in the CLI ($trace)"
                    } else {
                        # Pre-native plain foreground runs in the CLI.
                        Assert ((Test-Path $report) -and $result.Code -eq 7) "older server: plain command runs in the CLI with its exit code ($trace)"
                        Assert (@($observed | Where-Object { $_.Body -and $_.Body.Contains('reject-marker') }).Count -eq 0) "older server: plain command never sent to the server ($trace)"
                        Remove-Item $report
                    }
                    continue
                }
                Assert (-not (Test-Path $report)) 'peer rejection or bad reply never causes local marker execution'
                Assert ($observed.Count -eq 1) "run-shell uses one connection without reconnect ($trace)"
                if ($case.Submitted) {
                    $prefix = 'run-shell'; if ($background) { $prefix += ' -b' }; $prefix += ' -- '
                    Assert ($observed[0].Body.StartsWith('CLIENT_CWD ') -and $observed[0].Body.Contains("`n$prefix") -and $observed[0].Body.Contains('reject-marker.ps1')) 'bad final frame follows actual command submission to the nonexecuting peer'
                } else {
                    Assert ($observed[0].Body -ceq '') 'rejected ACK receives AUTH only, no cwd metadata or shell command'
                }
                Assert ($result.Code -ne 0) "peer failure returns nonzero ($($case.Name), background=$background)"
            } finally {
                $done.Set()
                $listener.Stop()
                $worker.Dispose()
                $done.Dispose()
                Remove-Item $portFile,$keyFile,$pidFile -ErrorAction SilentlyContinue
            }
        }
    }
    Remove-Item Env:PSMUX_TARGET_SESSION
    & $Binary -L $ns -f $conf new-session -d -s $session -c $serverCwd.FullName | Out-Null
    Assert ($LASTEXITCODE -eq 0) 'isolated session created'
    # Keep status line messages up long enough for dump-state polling.
    P set -g display-time 60000 | Out-Null
    $serverPid = [int]((P display-message -p '#{pid}') -join '').Trim()
    Assert ($serverPid -gt 0) 'server PID known before ownership check'
    $result = Run-Bytes "Start-Sleep -Seconds 4; [Console]::Out.Write('LATE_OUT'); [Console]::Error.Write('LATE_ERR'); exit 7"
    Assert ($result.Code -eq 7) "quiet foreground retains exit status beyond the three-second AUTH timeout (exit=$($result.Code), stderr=$([Text.Encoding]::UTF8.GetString($result.Err)))"
    Assert ([Text.Encoding]::UTF8.GetString($result.Out) -ceq 'LATE_OUT' -and [Text.Encoding]::UTF8.GetString($result.Err) -ceq 'LATE_ERR') 'quiet foreground retains separate output after the AUTH timeout'
    $env:PSMUX_JOB_PROBE = 'caller'
    Set-Location $callerCwd.FullName
    $probe = Join-Path $root 'probe.ps1'
    @'
param($Report, $Gate, $Token)
$parent = (Get-CimInstance Win32_Process -Filter "ProcessId=$PID").ParentProcessId
@{ Parent = $parent; Cwd = (Get-Location).Path; PwdEnv = $env:PWD; Env = $env:PSMUX_JOB_PROBE; Tmux = $env:TMUX } |
    ConvertTo-Json -Compress | Set-Content $Report
if ($Gate -ne '-') {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (Test-Path $Gate)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Test gate timed out' }
        Start-Sleep -Milliseconds 50
    }
}
[Console]::Write($Token)
[Console]::Error.Write('STDERR_' + $Token)
exit 7
'@ | Set-Content $probe
    function Probe-Command($report, $gatePath, $token) {
        return "pwsh -NoProfile -File '$probe' '$report' '$gatePath' '$token'"
    }
    # Plain CLI without format syntax proves actual execution ownership.
    $report = Join-Path $root 'foreground.json'
    Assert (-not (Test-Path $report)) 'foreground report starts absent'
    $cmd = Probe-Command $report '-' 'FG_OUTPUT'
    $out = (P run-shell $cmd) -join "`n"
    Assert ($LASTEXITCODE -eq 7) 'foreground retains exit status'
    $r = Get-Content $report -Raw | ConvertFrom-Json
    Assert ($r.Parent -eq $serverPid) 'plain foreground is owned by server'
    Assert ($out.Contains('FG_OUTPUT') -and $out.Contains('STDERR_FG_OUTPUT') -and -not $out.Contains('returned 7')) 'native CLI bytes do not include UI status text'
    Assert ($r.Env -eq 'server' -and $r.Tmux) 'job uses server environment and TMUX'
    Assert ($r.Cwd -eq $callerCwd.FullName) 'unattached CLI uses caller cwd'
    $out = (P run-shell 'cmd /c echo OK') -join "`n"
    Assert ($LASTEXITCODE -eq 0 -and $out.Trim() -eq 'OK') 'consumed AUTH acknowledgement does not strip child OK output'
    $hostname = Join-Path $env:SystemRoot 'System32/hostname.exe'
    Assert ((Test-Path $hostname -PathType Leaf) -and -not $hostname.Contains(' ')) 'redirection probe is an existing non-spaced executable path'
    $redirected = Join-Path $callerCwd.FullName 'hostname result.txt'
    Assert (-not (Test-Path $redirected)) 'redirection destination starts absent'
    $result = Run-Bytes "$hostname > '$redirected'"
    Assert ($result.Code -eq 0 -and $result.Out.Length -eq 0 -and $result.Err.Length -eq 0) 'foreground shell redirection succeeds without reply output'
    Assert (Test-Path $redirected -PathType Leaf) 'shell redirection creates the destination'
    Assert ((Get-Content $redirected -Raw).Trim() -eq [Net.Dns]::GetHostName()) 'redirected file contains the actual guest hostname'
    $raw = Join-Path $root 'raw.ps1'
    @'
$bytes = [byte[]](0..255)
$out = [Console]::OpenStandardOutput()
$err = [Console]::OpenStandardError()
for ($i = 0; $i -lt 256; $i++) { $out.Write($bytes, 0, $bytes.Length) }
[Array]::Reverse($bytes)
for ($i = 0; $i -lt 256; $i++) { $err.Write($bytes, 0, $bytes.Length) }
exit 7
'@ | Set-Content $raw
    $result = Run-Bytes "pwsh -NoProfile -File '$raw'"
    $expected = [byte[]]((0..255) * 256)
    $reverse = [byte[]](0..255); [Array]::Reverse($reverse)
    $expectedErr = [byte[]]($reverse * 256)
    Assert ($result.Code -eq 7) 'raw reply carries actual foreground status'
    Assert ([Convert]::ToBase64String($result.Out) -ceq [Convert]::ToBase64String($expected)) 'stdout retains NUL, invalid UTF-8, and pipe-sized binary output'
    Assert ([Convert]::ToBase64String($result.Err) -ceq [Convert]::ToBase64String($expectedErr)) 'stderr retains separate raw bytes'
    Assert ((Get-Location).ProviderPath -eq $callerCwd.FullName) 'PowerShell filesystem cwd is the caller cwd before relative execution'
    Assert ([Environment]::CurrentDirectory -ne $callerCwd.FullName) 'native process cwd differs from PowerShell caller cwd'
    $report = Join-Path $root 'native-cwd.json'
    Assert (-not (Test-Path $report)) 'native helper cwd report starts absent'
    $result = Run-Bytes (Probe-Command $report '-' 'NATIVE_CWD')
    Assert ($result.Code -eq 7 -and (Test-Path $report)) "native helper executes absolute cwd probe (Code=$($result.Code); Err=$(ConvertTo-Json -Compress -InputObject ([Text.Encoding]::UTF8.GetString([byte[]]$result.Err))))"
    $r = Get-Content $report -Raw | ConvertFrom-Json
    Assert ($r.Parent -eq $serverPid -and $r.Cwd -eq $callerCwd.FullName) 'native client supplies PowerShell caller cwd to the server-owned job'
    $relative = Join-Path $callerCwd.FullName 'relative probe.ps1'
    Copy-Item $probe $relative
    Assert (-not (Test-Path (Join-Path ([Environment]::CurrentDirectory) 'relative probe.ps1'))) 'relative script is absent from the native process cwd'
    Assert (-not (Test-Path (Join-Path $serverCwd.FullName 'relative probe.ps1'))) 'relative spaced script exists only in caller cwd'
    $report = Join-Path $root 'relative.json'
    $result = Run-Bytes "relative probe.ps1 '$report' '-' 'RELATIVE_OUTPUT'"
    Assert ($result.Code -eq 7 -and (Test-Path $report)) "relative spaced ps1 is probed in job cwd (Code=$($result.Code); Out=$(ConvertTo-Json -Compress -InputObject ([Text.Encoding]::UTF8.GetString([byte[]]$result.Out))); Err=$(ConvertTo-Json -Compress -InputObject ([Text.Encoding]::UTF8.GetString([byte[]]$result.Err))))"
    $r = Get-Content $report -Raw | ConvertFrom-Json
    Assert ($r.Parent -eq $serverPid -and $r.Cwd -eq $callerCwd.FullName) 'relative script is still a server-owned job'
    # Explicit -c wins over native caller metadata, including HOME fallback.
    foreach ($dir in $explicitCwd.FullName,$jobHome.FullName) {
        Copy-Item $probe (Join-Path $dir 'cwdprobe.ps1')
    }
    Assert (-not (Test-Path (Join-Path $callerCwd.FullName 'cwdprobe.ps1'))) 'cwd probe is absent from caller cwd'
    Assert (-not (Test-Path (Join-Path $serverCwd.FullName 'cwdprobe.ps1'))) 'cwd probe is absent from server cwd'
    $missingCwd = Join-Path $root 'missing cwd'
    Assert (-not (Test-Path $missingCwd)) 'invalid start directory does not exist'
    foreach ($case in @(
        @{ Name = 'explicit'; Cwd = $explicitCwd.FullName; Expected = $explicitCwd.FullName },
        @{ Name = 'relative-c'; Cwd = 'explicit cwd'; Expected = $explicitCwd.FullName },
        @{ Name = 'missing-c'; Cwd = $missingCwd; Expected = $jobHome.FullName },
        @{ Name = 'tilde-c'; Cwd = '~'; Expected = $jobHome.FullName }
    )) {
        foreach ($background in $false,$true) {
            $tag = "$($case.Name)-$background"
            $report = Join-Path $root "$tag.json"
            $gate = Join-Path $root "$tag.gate"
            Assert (-not (Test-Path $report) -and -not (Test-Path $gate)) 'start-directory report and gate begin absent'
            $flags = @('-c', $case.Cwd)
            if ($background) {
                $flags = @('-b') + $flags
                Launch "cwdprobe.ps1 '$report' '$gate' '$tag'" $flags
                Assert (Wait-Until { Bg-Failure-Shown $tag "STDERR_$tag" }) 'start-directory background failure on the status line, output not shown'
            } else {
                $result = Run-Bytes "cwdprobe.ps1 '$report' '-' '$tag'" $flags
                Assert ($result.Code -eq 7 -and [Text.Encoding]::UTF8.GetString($result.Out) -ceq $tag -and [Text.Encoding]::UTF8.GetString($result.Err) -ceq "STDERR_$tag") 'start-directory foreground reply retains separate output and actual status'
            }
            $r = Get-Content $report -Raw | ConvertFrom-Json
            Assert ($r.Parent -eq $serverPid -and $r.Env -eq 'server' -and $r.Cwd -eq $case.Expected) "server probes relative file in effective job cwd ($tag)"
        }
    }
    $nativeProbe = Join-Path $explicitCwd.FullName 'nativeprobe.exe'
    Copy-Item $hostname $nativeProbe
    Assert (-not (Test-Path (Join-Path $callerCwd.FullName 'nativeprobe.exe'))) 'relative native executable exists only in explicit cwd'
    $result = Run-Bytes '.\nativeprobe.exe' @('-c', $explicitCwd.FullName)
    Assert ($result.Code -eq 0 -and [Text.Encoding]::UTF8.GetString($result.Out).Trim() -eq [Net.Dns]::GetHostName() -and $result.Err.Length -eq 0) 'relative native executable works with explicit cwd containing spaces'
    $nativeRedirect = Join-Path $explicitCwd.FullName 'native result.txt'
    Assert (-not (Test-Path $nativeRedirect)) 'explicit-c native redirection destination starts absent'
    $result = Run-Bytes ".\nativeprobe.exe > 'native result.txt'" @('-c', $explicitCwd.FullName)
    Assert ($result.Code -eq 0 -and $result.Out.Length -eq 0 -and $result.Err.Length -eq 0 -and (Test-Path $nativeRedirect)) 'relative native executable still uses shell redirection with explicit cwd'
    Assert ((Get-Content $nativeRedirect -Raw).Trim() -eq [Net.Dns]::GetHostName()) 'explicit-c redirected file contains actual hostname'
    $git = Get-Command git -ErrorAction Stop
    $bash = Join-Path (Split-Path (Split-Path $git.Source)) 'bin/bash.exe'
    Assert (Test-Path $bash) 'VM provides Git for Windows for tmux script execution'
    $entry = Join-Path $callerCwd.FullName 'relative entry.tmux'
    [IO.File]::WriteAllText($entry, "#!/bin/sh`nprintf TMUX_RELATIVE_OK`nexit 7`n", [Text.UTF8Encoding]::new($false))
    Assert (-not (Test-Path (Join-Path $serverCwd.FullName 'relative entry.tmux'))) 'relative tmux entry exists only in caller cwd'
    $result = Run-Bytes 'relative entry.tmux'
    Assert ($result.Code -eq 7 -and [Text.Encoding]::UTF8.GetString($result.Out) -eq 'TMUX_RELATIVE_OK') 'relative spaced tmux entry uses job cwd'
    Assert (((P display-message -p '#{session_path}') -join '').Trim() -eq $serverCwd.FullName) 'job construction does not change process-wide server cwd'
    $badExe = Join-Path $callerCwd.FullName 'invalid exe.exe'
    [IO.File]::WriteAllBytes($badExe, [byte[]](1,2,3,4))
    $result = Run-Bytes "'$badExe'" @('-b')
    Assert ($result.Code -ne 0 -and $result.Out.Length -eq 0 -and $result.Err.Length -gt 0) 'immediate background invalid executable fails launch'
    P display-message -d 5000 RESET_ERROR | Out-Null
    Assert ((Server-State).status_message -eq 'RESET_ERROR') 'previous spawn error is replaced before delayed failure'
    $result = Run-Bytes "'$badExe'" @('-b', '-d', '0.5')
    Assert ($result.Code -eq 0 -and $result.Out.Length -eq 0 -and $result.Err.Length -eq 0) 'delayed background reply acknowledges scheduling'
    Assert (Wait-Until { (Server-State).status_message -like 'run-shell:*' }) 'delayed spawn failure reaches server display'
    foreach ($flag in '-b', '-bd') {
        $flags = @($flag)
        if ($flag -eq '-bd') { $flags += '0.1' }
        $tag = [guid]::NewGuid().ToString('N')
        $report = Join-Path $root "$tag.json"
        $gate = Join-Path $root "$tag.gate"
        Assert (-not (Test-Path $gate) -and -not (Test-Path $report)) 'background gate and report start absent'
        $cmd = Probe-Command $report $gate "BG_$tag"
        Launch $cmd $flags
        Assert (Wait-Until { Test-Path $report }) 'background probe starts'
        Assert (Wait-Until { Bg-Failure-Shown "BG_$tag" "STDERR_BG_$tag" }) 'background failure on the status line, output not shown'
        $r = Get-Content $report -Raw | ConvertFrom-Json
        Assert ($r.Parent -eq $serverPid -and $r.Env -eq 'server') 'background server ownership and env'
        Assert ($r.Cwd -eq $callerCwd.FullName) 'background caller cwd survives client exit'
    }
    # Control mode is an attached client: cwd is the session start directory.
    $report = Join-Path $root 'control.json'
    $gate = Join-Path $root 'control.gate'
    Assert (-not (Test-Path $report) -and -not (Test-Path $gate)) 'control probe starts absent'
    $wireCommand = $null
    $block = @()
    $invalidBlock = @()
    $lastControlState = [ref]$null
    try {
        $c = Connect 'CONTROL_QUIET'
        try {
            # Consume attach's initial block before sending the test command.
            do {
                $line = $c.Reader.ReadLine()
                if ($null -eq $line) { throw 'EOF before control attach block ends' }
            } while ($line -notmatch '^%(end|error) ')
            $cmd = Probe-Command $report $gate 'CONTROL_OUTPUT'
            $wireCommand = 'run-shell -b "' + $cmd.Replace('\','\\').Replace('"','\"') + '"'
            $c.Writer.WriteLine($wireCommand)
            do {
                $line = $c.Reader.ReadLine()
                if ($null -eq $line) { throw 'EOF before run-shell launch block ends' }
                $block += $line
            } while ($line -notmatch '^%(end|error) ')
            Assert ($line -match '^%end ') 'control -b reports launch success before gate opens'
            Assert (($block -join "`n") -notmatch 'CONTROL_OUTPUT|returned 7') 'control launch block does not contain final output/status'
            Assert (Wait-Until { (Test-Path $report) -and -not (Test-Path $gate) }) 'control probe starts while gate remains closed'
            $c.Writer.WriteLine("run-shell -b `"$($badExe.Replace('\','\\'))`"")
            do {
                $line = $c.Reader.ReadLine()
                if ($null -eq $line) { throw 'EOF before invalid executable launch reply' }
                $invalidBlock += $line
            } while ($line -notmatch '^%(end|error) ')
            Assert ($line -match '^%error ') 'control invalid executable reports spawn failure'
        } finally {
            New-Item -ItemType File -Force $gate | Out-Null
            $c.Tcp.Dispose()
        }
        Assert (Wait-Until {
            $lastControlState.Value = Server-State
            $text = $lastControlState.Value.popup_lines -join "`n"
            $status = "$($lastControlState.Value.status_message)"
            $status.Contains('CONTROL_OUTPUT') -and $status.Contains('returned 7') -and -not $text.Contains('STDERR_CONTROL_OUTPUT')
        }) 'control background failure on the status line, output not shown'
        $r = Get-Content $report -Raw | ConvertFrom-Json
        Assert ($r.Cwd -eq $serverCwd.FullName -and $r.Parent -eq $serverPid) 'attached control uses session cwd and server ownership'
    } catch {
        try {
            Write-Host "Control wire (JSON): $(ConvertTo-Json -Compress -InputObject $wireCommand)"
            Write-Host "Control launch reply (JSON): $(ConvertTo-Json -Compress -InputObject $block)"
            Write-Host "Control invalid executable reply (JSON): $(ConvertTo-Json -Compress -InputObject $invalidBlock)"
            $reportExists = Test-Path $report
            $reportText = if ($reportExists) { Get-Content $report -Raw } else { $null }
            Write-Host "Control report/gate (JSON): $(ConvertTo-Json -Compress -InputObject @{ ReportExists = $reportExists; Report = $reportText; GateExists = (Test-Path $gate) })"
            if ($null -eq $lastControlState.Value) { $lastControlState.Value = Server-State }
            Write-Host "Control final popup/status (JSON): $(ConvertTo-Json -Compress -InputObject @{ Popup = $lastControlState.Value.popup_lines; Status = $lastControlState.Value.status_message })"
        } catch { Write-Host "Control diagnostics failed: $_" }
        throw
    }
    # Hook route exercises the in-process commands.rs implementation.
    $report = Join-Path $root 'hook.json'
    $cmd = Probe-Command $report '-' 'HOOK_OUTPUT'
    P set-hook -g after-new-window "run-shell -b `"$cmd`"" | Out-Null
    Assert ($LASTEXITCODE -eq 0 -and -not (Test-Path $report)) 'hook configured without executing probe'
    P new-window -d | Out-Null
    Assert ($LASTEXITCODE -eq 0) 'hook event succeeds'
    Assert (Wait-Until { Bg-Failure-Shown 'HOOK_OUTPUT' 'STDERR_HOOK_OUTPUT' }) 'hook background failure on the status line, output not shown'
    P run-shell -b 'cmd /c exit 9' | Out-Null
    Assert ($LASTEXITCODE -eq 0) 'silent failing background job returns launch success'
    Assert (Wait-Until { "$((Server-State).status_message)".Contains("'cmd /c exit 9' returned 9") }) 'silent background failure is on the status line'
    Assert (-not (Server-State).popup_active) 'no -b job opened a popup'
    # Delayed commandless jobs still wait, and invalid delay remains an error.
    $timer = [Diagnostics.Stopwatch]::StartNew()
    P run-shell -d 0.2 | Out-Null
    Assert ($LASTEXITCODE -eq 0 -and $timer.ElapsedMilliseconds -ge 150) 'commandless delay waits server-side'
    P run-shell -d invalid | Out-Null
    Assert ($LASTEXITCODE -ne 0) 'invalid delay rejected'
    # Kill only this namespace after a foreground command has been submitted.
    # A local retry would write a second record and block on the closed gate.
    $once = Join-Path $root 'once.ps1'
    @'
param($Count, $Gate)
Add-Content $Count $PID
while (-not (Test-Path $Gate)) { Start-Sleep -Milliseconds 50 }
exit 7
'@ | Set-Content $once
    $count = Join-Path $root 'once.txt'; $gate = Join-Path $root 'once.gate'
    Assert (-not (Test-Path $count) -and -not (Test-Path $gate)) 'disconnect count and gate start absent'
    $info = [Diagnostics.ProcessStartInfo]::new($Binary)
    $info.WorkingDirectory = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    foreach ($arg in @('-L', $ns, 'run-shell', "pwsh -NoProfile -File '$once' '$count' '$gate'")) { $info.ArgumentList.Add($arg) }
    $client = [Diagnostics.Process]::Start($info)
    try {
        Assert (Wait-Until { Test-Path $count }) 'foreground command submitted and running'
        Assert (@(Get-Content $count).Count -eq 1) 'one execution before disconnect'
        P kill-server | Out-Null
        Assert ($client.WaitForExit(6000) -and $client.ExitCode -ne 0) 'lost submitted reply fails instead of local fallback'
        Assert (@(Get-Content $count).Count -eq 1) 'no second execution after disconnect'
    } finally {
        New-Item -ItemType File -Force $gate | Out-Null
        if (-not $client.HasExited) { $null = $client.WaitForExit(6000) }
        if (-not $client.HasExited) { $client.Kill() }
        $client.Dispose()
    }
} catch {
    $primaryFailure = $_
    throw
} finally {
    try {
        Set-Location $originalCwd
        if (Test-Path (Join-Path $root 'data')) { P kill-server | Out-Null }
        foreach ($name in $saved.Keys) {
            if ($null -eq $saved[$name]) {
                [Environment]::SetEnvironmentVariable($name, [System.Management.Automation.Language.NullString]::Value)
            } else {
                [Environment]::SetEnvironmentVariable($name, $saved[$name])
            }
        }
        $after = Default-Sessions
        if ($before -cne $after) {
            Write-Host "Default sessions before (JSON): $(ConvertTo-Json -Compress -InputObject $before)"
            Write-Host "Default sessions after (JSON): $(ConvertTo-Json -Compress -InputObject $after)"
        }
        Assert ($before -ceq $after) 'default session list unchanged'
    } catch {
        if ($null -eq $primaryFailure) { throw }
        Write-Host "FAIL: cleanup after primary failure: $($_ | Out-String)"
    } finally {
        # Keep reports and diagnostics for the VM operator; only namespace server is stopped.
        Write-Host "Artifacts: $root"
    }
}
