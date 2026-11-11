param(
    [string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }),
    [int]$ThreadLimit = 400,
    [int]$TimeoutMs = 8000,
    [int]$SettleMs = 1500,
    [int]$ReadCallMs = 2000,
    [int]$ListCallMs = 5000,
    [int]$StartCallMs = 15000,
    [int]$QueryCallMs = 5000,
    [int]$LaterCallMs = 15000,
    [int]$CleanupCallMs = 10000
)

$ErrorActionPreference = "Continue"
$Exe = (Resolve-Path $Exe).Path
$script:Passed = 0
$script:Failed = 0
function Write-Pass($message) { Write-Host "  [PASS] $message" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($message) { Write-Host "  [FAIL] $message" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $message, $detail) { if ($ok) { Write-Pass $message } else { Write-Fail "$message ($detail)" } }

function New-ProcessStart([string[]]$words) {
    $start = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($word in $words) { $start.ArgumentList.Add($word) }
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start
}

function Invoke-Bounded([int]$limitMs, [string[]]$words) {
    $process = [Diagnostics.Process]::Start((New-ProcessStart $words))
    $output = $process.StandardOutput.ReadToEndAsync()
    $errors = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($limitMs)) {
        try { $process.Kill() } catch { }
        throw "psmux $($words -join ' ') timed out after $limitMs ms"
    }
    foreach ($reader in $output, $errors) {
        if (-not $reader.Wait($ReadCallMs)) { throw "psmux $($words -join ' ') left its output open for $ReadCallMs ms" }
    }
    @{ Exit = $process.ExitCode; Text = $output.Result.Trim(); Errors = $errors.Result.Trim() }
}

function Invoke-Test([int]$limitMs, [string[]]$words) {
    Invoke-Bounded $limitMs (@('-L', $script:namespace) + $words)
}

function Get-DefaultSessions {
    $listed = Invoke-Bounded $ListCallMs @('ls', '-F', '#{session_name} #{session_created}')
    if ($listed.Exit -ne 0) {
        if ($listed.Text -or $listed.Errors -notmatch '^psmux: no server running on ([^\r\n]+)$') {
            throw "ls exit $($listed.Exit), output '$($listed.Text)', errors '$($listed.Errors)'"
        }
        try { [void][IO.Directory]::GetFileSystemEntries($Matches[1]) } catch [IO.DirectoryNotFoundException] { }
        return ''
    }
    (($listed.Text -split "`r?`n") | Sort-Object) -join "`n"
}

function Send-Line($session, $line) {
    $base = "$($script:namespace)__$session"
    $port = (Get-Content (Join-Path $script:data "$base.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $script:data "$base.key") -Raw).Trim()
    $client = [Net.Sockets.TcpClient]::new()
    try {
        if (-not $client.ConnectAsync("127.0.0.1", [int]$port).Wait($QueryCallMs)) { throw "connect to $session timed out after $QueryCallMs ms" }
        $client.SendTimeout = $QueryCallMs
        $stream = $client.GetStream()
        $bytes = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
        $stream.Write($bytes, 0, $bytes.Length)
        $client.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
        $reply = [IO.StreamReader]::new($stream).ReadToEndAsync()
        if (-not $reply.Wait($QueryCallMs)) { throw "'$line' on $session timed out after $QueryCallMs ms" }
        $reply.Result
    } finally {
        $client.Close()
    }
}

function Get-TestProcesses {
    @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.ExecutablePath -eq $Exe -and $_.CommandLine -match "-L $($script:namespace)(\s|$)" })
}

function Stop-TestServer {
    try {
        if ((Get-TestProcesses).Count -gt 0) { Invoke-Test $CleanupCallMs @('kill-server') | Out-Null }
    } catch {
        Write-Fail "cleanup of $($script:namespace): $_"
    }
    Start-Sleep -Milliseconds 300
    foreach ($process in (Get-TestProcesses)) { Stop-Process -Id $process.ProcessId -Force -EA SilentlyContinue }
}

function Start-TestServer($sessions) {
    $script:namespace = "shglobal$(Get-Random)"
    $script:data = Join-Path $script:root $script:namespace
    New-Item -ItemType Directory $script:data -Force | Out-Null
    $env:PSMUX_DATA_DIR = $script:data
    $empty = Join-Path $script:data "empty.conf"
    "" | Set-Content $empty
    $env:PSMUX_CONFIG_FILE = $empty
    $env:PSMUX_SERVER_MODE = "shared"
    foreach ($session in $sessions) {
        $made = Invoke-Test $StartCallMs @('new-session', '-d', '-s', $session, '-x', '120', '-y', '30')
        if ($made.Exit -ne 0) { throw "new-session $session exit $($made.Exit)" }
    }
    Start-Sleep -Milliseconds 1500
    $reported = (Invoke-Test $StartCallMs @('display', '-p', '-t', 'A', '#{pid}')).Text
    $server = Get-Process -Id ([int]($reported -replace '\D', '')) -EA Stop
    if ($server.Path -ne $Exe) { throw "the server is $($server.Path), not $Exe" }
    $server
}

function Watch-Line($server, [string[]]$words) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $cli = [Diagnostics.Process]::Start((New-ProcessStart (@('-L', $script:namespace) + $words)))
    $result = @{ Peak = 0; OverLimit = $false; ServerGone = $false; CliMs = -1; CliExit = $null; Settled = $false }
    while ($true) {
        $server.Refresh()
        if ($server.HasExited) { $result.ServerGone = $true; break }
        $threads = $server.Threads.Count
        if ($threads -gt $result.Peak) { $result.Peak = $threads }
        if ($threads -gt $ThreadLimit) { $result.OverLimit = $true; break }
        if ($result.CliMs -lt 0 -and $cli.HasExited) {
            $result.CliMs = $clock.ElapsedMilliseconds
            $result.CliExit = $cli.ExitCode
        }
        if ($result.CliMs -ge 0 -and ($clock.ElapsedMilliseconds - $result.CliMs) -ge $SettleMs) { $result.Settled = $true; break }
        if ($clock.ElapsedMilliseconds -ge $TimeoutMs) { break }
        Start-Sleep -Milliseconds 20
    }
    if ($result.OverLimit -and -not $server.HasExited) { $server.Kill() }
    if (-not $cli.HasExited) { $cli.Kill() }
    $result
}

function Count-Matches($session, $query, $pattern) {
    ([regex]::Matches((Send-Line $session $query), $pattern)).Count
}

function Test-Line($sessions, [string[]]$words, $query, $pattern, $seed) {
    $label = "$($sessions -join '') '$($words -join ' ')'"
    try {
        $server = Start-TestServer $sessions
        if ($seed) { Send-Line A $seed | Out-Null }
        $watched = Watch-Line $server $words
        if ($watched.OverLimit) { Write-Fail "$label ran away past $ThreadLimit threads"; return }
        if ($watched.ServerGone) { Write-Fail "$label ended the server"; return }
        if ($watched.CliMs -lt 0) { Write-Fail "$label timed out after $TimeoutMs ms (peak $($watched.Peak) threads)"; return }
        if (-not $watched.Settled) { Write-Fail "$label was not watched for $SettleMs ms after the CLI ended"; return }
        Check ($watched.CliExit -eq 0) "$label exits 0 ($($watched.CliMs) ms, peak $($watched.Peak) threads)" "exit $($watched.CliExit)"
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $reply = (Invoke-Test $QueryCallMs @('display', '-p', '-t', 'A', 'pong')).Text
        Check ($reply -eq 'pong' -and $clock.ElapsedMilliseconds -lt 2000) "$label the server answers ($($clock.ElapsedMilliseconds) ms)" "reply '$reply'"
        foreach ($session in $sessions) {
            $count = Count-Matches $session $query $pattern
            Check ($count -eq 1) "$label is in $session once" "seen $count times"
        }
        $later = Invoke-Test $LaterCallMs @('new-session', '-d', '-s', 'L', '-x', '120', '-y', '30')
        if ($later.Exit -ne 0) { throw "new-session L exit $($later.Exit)" }
        $count = Count-Matches L $query $pattern
        Check ($count -eq 1) "$label is in a later session once" "seen $count times"
    } catch {
        Write-Fail "$label threw: $_"
    } finally {
        Stop-TestServer
    }
}

function Test-PlainWrites($sessions) {
    $label = "$($sessions -join '') plain writes"
    try {
        Start-TestServer $sessions | Out-Null
        Send-Line A 'set -g @added x' | Out-Null
        Send-Line A 'set -ga @added y' | Out-Null
        Send-Line A 'set -g @same 1' | Out-Null
        Send-Line B 'set -g @same 2' | Out-Null
        Send-Line A 'set -g @same 1' | Out-Null
        $later = Invoke-Test $LaterCallMs @('new-session', '-d', '-s', 'L', '-x', '120', '-y', '30')
        if ($later.Exit -ne 0) { throw "new-session L exit $($later.Exit)" }
        foreach ($session in @($sessions) + 'L') {
            $added = (Send-Line $session 'show -gv @added') -replace '^OK\s*', ''
            Check ($added.Trim() -eq 'xy') "$label set -ga is applied once in $session" "'$added'"
            $same = (Send-Line $session 'show -gv @same') -replace '^OK\s*', ''
            Check ($same.Trim() -eq '1') "$label a line repeated by hand still reaches $session" "'$same'"
        }
    } catch {
        Write-Fail "$label threw: $_"
    } finally {
        Stop-TestServer
    }
}

$inherited = @{}
foreach ($name in 'TEMP', 'TMP', 'USERPROFILE', 'HOME', 'PSMUX_PSREADLINE_HISTORY', 'PSMUX_DATA_DIR', 'PSMUX_SERVER_MODE', 'PSMUX_CONFIG_FILE') {
    $inherited[$name] = [Environment]::GetEnvironmentVariable($name)
}
foreach ($name in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME', 'PSMUX_DATA_DIR', 'PSMUX_SERVER_MODE') {
    Remove-Item "Env:\$name" -EA SilentlyContinue
}
$defaultBefore = $null
try {
    $defaultBefore = Get-DefaultSessions
} catch {
    Write-Fail "the default session list: $_"
}

$script:root = Join-Path ([IO.Path]::GetTempPath()) "psmux_shglobal_$(Get-Random)"
try {
    if ($null -ne $defaultBefore) {
        foreach ($folder in 'home', 'temp') { New-Item -ItemType Directory (Join-Path $script:root $folder) -Force | Out-Null }
        $env:USERPROFILE = Join-Path $script:root 'home'
        $env:HOME = $env:USERPROFILE
        $env:TEMP = Join-Path $script:root 'temp'
        $env:TMP = $env:TEMP
        $env:PSMUX_PSREADLINE_HISTORY = Join-Path $script:root 'history.txt'

        Write-Host "`n=== shared server wide writes reach every session once (#798) ===" -ForegroundColor Cyan
        $bindQuery = 'list-keys -T root'
        $hookQuery = 'show-hooks -g'
        foreach ($sessions in @(@('A', 'B'), @('A', 'B', 'D'))) {
            Test-Line $sessions @('bind', '-T', 'root', 'F8', 'send-keys', '-t', 'B:', '-l', 'XQ') $bindQuery '(?m)^bind-key\s+-T root\s+F8\s+send-keys -t B: -l XQ\s*$'
            Test-Line $sessions @('bind', '-n', 'F7', 'kill-window', '-t', 'B:1') $bindQuery '(?m)^bind-key\s+-T root\s+F7\s+kill-window -t B:1\s*$'
            Test-Line $sessions @('bind', '-n', 'F7', 'send-keys', '-t', 'B:', '-l', 'XQ') $bindQuery '(?m)^bind-key\s+-T root\s+F7\s+send-keys -t B: -l XQ\s*$'
            Test-Line $sessions @('set-hook', '-g', 'after-new-window', 'new-window', '-t', 'B:') $hookQuery '(?m)^after-new-window\[0\] new-window -t B:\s*$'
            Test-Line $sessions @('set', '-g', '@tagged', 'value', '-t', 'B:') 'show -gv @tagged' '(?m)^value\s*$'
            Test-Line $sessions @('set', '-ga', '@tagged', 'y', '-t', 'B:') 'show -gv @tagged' '(?m)^xy\s*$' 'set -g @tagged x'
            Test-Line $sessions @('set', '-ga', '-t', 'B:', '@tagged', 'y') 'show -gv @tagged' '(?m)^xy\s*$' 'set -g @tagged x'
            Test-PlainWrites $sessions
        }
    }
} catch {
    Write-Fail "the suite threw: $_"
} finally {
    foreach ($name in $inherited.Keys) {
        if ($null -eq $inherited[$name]) { Remove-Item "Env:\$name" -EA SilentlyContinue } else { Set-Item "Env:\$name" $inherited[$name] }
    }
    Remove-Item -Recurse -Force $script:root -EA SilentlyContinue
}

try {
    $keepData = $env:PSMUX_DATA_DIR
    Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
    $defaultAfter = Get-DefaultSessions
    if ($keepData) { $env:PSMUX_DATA_DIR = $keepData }
    Check ($null -ne $defaultBefore -and $defaultAfter -ceq $defaultBefore) "the default sessions are unchanged" "'$defaultBefore' -> '$defaultAfter'"
} catch {
    Write-Fail "the default session list: $_"
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Passed)"
Write-Host "  Failed: $($script:Failed)"
exit $script:Failed
