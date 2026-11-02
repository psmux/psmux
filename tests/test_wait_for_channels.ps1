# wait-for CLI, command-list, control and source-job completion.
# PSMUX_EXE selects the binary; the fixture owns only its unique -L namespace.
$ErrorActionPreference = 'Stop'
$exe = $env:PSMUX_EXE
if (-not $exe) { $exe = Join-Path $PSScriptRoot '..\target\release\psmux.exe' }
$exe = (Resolve-Path $exe).Path
$ns = 'waitfor-' + [guid]::NewGuid().ToString('N')
$children = [Collections.Generic.List[object]]::new()
$sockets = [Collections.Generic.List[object]]::new()
$temp = Join-Path ([IO.Path]::GetTempPath()) $ns
[IO.Directory]::CreateDirectory($temp) | Out-Null
$config = Join-Path $temp 'empty.conf'
[IO.File]::WriteAllText($config, '')

function Check($condition, $message) { if (-not $condition) { throw $message } }
function Start-CLI([string[]]$Words, [bool]$Default = $false) {
    $info = [Diagnostics.ProcessStartInfo]::new($exe)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($key in @($info.Environment.Keys)) {
        if (($key -like 'PSMUX_*' -and $key -notin @('PSMUX_DATA_DIR', 'PSMUX_EXE')) -or $key -like 'TMUX*') {
            $info.Environment.Remove($key) | Out-Null
        }
    }
    $info.Environment['PSMUX_NO_WARM'] = '1'
    if (-not $Default) { $info.ArgumentList.Add('-L'); $info.ArgumentList.Add($ns) }
    foreach ($word in $Words) { $info.ArgumentList.Add($word) }
    $process = [Diagnostics.Process]::Start($info)
    $child = [pscustomobject]@{ Process = $process; Words = $Words; Out = $process.StandardOutput.ReadToEndAsync(); Err = $process.StandardError.ReadToEndAsync() }
    $children.Add($child)
    return $child
}
function Finish($child) {
    Check ($child.Process.WaitForExit(15000)) 'CLI did not exit within 15 seconds'
    return [pscustomobject]@{ Code = $child.Process.ExitCode; Out = $child.Out.Result; Err = $child.Err.Result }
}
function P {
    $result = Finish (Start-CLI -Words $args)
    Check ($result.Code -eq 0) "CLI failed: $args / $($result.Code) / $($result.Err)"
    return $result.Out.TrimEnd("`r", "`n")
}
function Pending($child, [int]$milliseconds = 300) {
    $wasRunning = -not $child.Process.HasExited
    if (-not $wasRunning -or $child.Process.WaitForExit($milliseconds)) {
        $result = Finish $child
        throw "wait completed without release: $($child.Words -join ' ') / initially_running=$wasRunning / exit=$($result.Code) / stdout=$($result.Out) / stderr=$($result.Err)"
    }
}
function Completed($child, [string]$expected = '') {
    $result = Finish $child
    Check ($result.Code -eq 0) "wait failed: $($result.Err)"
    Check ($result.Out.TrimEnd("`r", "`n") -ceq $expected) "unexpected output: '$($result.Out)'"
}
function Snapshot {
    $result = Finish (Start-CLI -Default $true -Words @('list-sessions', '-F', '#{session_name}'))
    Check ($result.Code -eq 0 -or $result.Err -match 'no server') "default namespace listing failed: $($result.Err)"
    return "$($result.Code):" + (($result.Out -split '\r?\n' | Where-Object { $_ } | Sort-Object) -join '|')
}
function Open-Raw([string]$mode) {
    $root = $env:PSMUX_DATA_DIR
    if (-not $root) { $root = Join-Path $env:USERPROFILE '.psmux' }
    $base = Join-Path $root ($ns + '__test')
    $port = [int]([IO.File]::ReadAllText($base + '.port').Trim())
    $key = [IO.File]::ReadAllText($base + '.key').Trim()
    $socket = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $sockets.Add($socket)
    $stream = $socket.GetStream()
    $stream.ReadTimeout = 5000
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false), 1024, $true)
    $writer.NewLine = "`n"
    $writer.AutoFlush = $true
    $reader = [IO.StreamReader]::new($stream, [Text.UTF8Encoding]::new($false), $false, 1024, $true)
    $writer.WriteLine('AUTH ' + $key)
    Check ($reader.ReadLine() -ceq 'OK') 'ordinary AUTH acknowledgement changed'
    $writer.WriteLine($mode)
    return [pscustomobject]@{ Socket = $socket; Stream = $stream; Writer = $writer; Reader = $reader }
}
function Until($raw, [string]$pattern) {
    $seen = [Collections.Generic.List[string]]::new()
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        Check ($timer.ElapsedMilliseconds -lt 10000) "control reply did not reach $pattern"
        $line = $raw.Reader.ReadLine()
        Check ($null -ne $line) 'unexpected control EOF'
        Check ($line -notmatch '^%error') "control command failed: $line"
        $seen.Add($line)
    } until ($line -match $pattern)
    return $seen.ToArray()
}
function No-Completion($raw, [string]$completionPattern = '^%(end|error)') {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        while ($timer.ElapsedMilliseconds -lt 300) {
            $raw.Stream.ReadTimeout = [Math]::Max(1, 300 - [int]$timer.ElapsedMilliseconds)
            try { $line = $raw.Reader.ReadLine() }
            catch [IO.IOException] {
                # Only a socket read timeout proves the block is still pending.
                $cause = $_.Exception
                while ($cause -and $cause -isnot [Net.Sockets.SocketException]) { $cause = $cause.InnerException }
                Check ($cause -is [Net.Sockets.SocketException] -and
                    $cause.SocketErrorCode -eq [Net.Sockets.SocketError]::TimedOut) 'read failed instead of remaining pending'
                return
            }
            Check ($null -ne $line) 'connection closed while waiting'
            Check ($line -notmatch $completionPattern) "wait finished before release: $line"
        }
    } finally { $raw.Stream.ReadTimeout = 5000 }
}

$before = Snapshot
try {
    P -f $config new-session -d -s test | Out-Null
    foreach ($channel in @('two words', '-dash', '')) {
        P wait-for -S '--' $channel | Out-Null
        P wait '--' $channel | Out-Null
    }
    P set-option -s 'command-alias[100]' 'await=wait-for' | Out-Null
    $aliasWait = Start-CLI -Words @('await', 'alias-ready')
    Pending $aliasWait
    P wait-for -S alias-ready | Out-Null
    Completed $aliasWait

    $first = Start-CLI -Words @('wait-for', 'broadcast')
    Pending $first 3500
    $second = Start-CLI -Words @('wait-for', 'broadcast')
    Pending $second
    Check ((P display-message -p responsive) -ceq 'responsive') 'server stalled behind wait'
    P wait-for -S broadcast | Out-Null
    Completed $first
    Completed $second
    $later = Start-CLI -Words @('wait-for', 'broadcast')
    Pending $later
    P wait-for -S broadcast | Out-Null
    Completed $later

    P wait-for -L mutex | Out-Null
    $lock1 = Start-CLI -Words @('wait-for', '-L', 'mutex')
    Pending $lock1
    $lock2 = Start-CLI -Words @('wait-for', '-L', 'mutex')
    Pending $lock2
    $wait = Start-CLI -Words @('wait-for', 'mutex')
    Pending $wait
    P wait-for -U mutex | Out-Null
    Completed $lock1
    Pending $lock2
    Pending $wait
    P wait-for -U mutex | Out-Null
    Completed $lock2
    P wait-for -U mutex | Out-Null
    Pending $wait
    P wait-for -S mutex | Out-Null
    Completed $wait
    $bad = Finish (Start-CLI -Words @('wait-for', '-U', 'mutex', ';', 'display-message', '-p', 'MUST-NOT-RUN'))
    Check ($bad.Code -ne 0 -and $bad.Err -match 'not locked' -and $bad.Out -notmatch 'MUST-NOT-RUN') 'unlock error did not stop its list'

    $chain = Start-CLI -Words @('display-message', '-p', 'BEFORE', ';', 'wait-for', 'chain', ';', 'display-message', '-p', 'AFTER')
    Pending $chain 3500
    P wait-for -S chain | Out-Null
    Completed $chain "BEFORE`nAFTER"

    P set-option -g '@after-source' before | Out-Null
    $source = Join-Path $temp 'source.conf'
    [IO.File]::WriteAllText($source, "wait-for source`nset-option -g @after-source after`n")
    $job = Start-CLI -Words @('source-file', $source)
    Pending $job
    Check ((P show-options -gv '@after-source') -ceq 'before') 'source job ran past its wait'
    P wait-for -S source | Out-Null
    Completed $job
    Check ((P show-options -gv '@after-source') -ceq 'after') 'source job did not resume after signal'

    $control = Open-Raw CONTROL_QUIET
    Until $control '^%end' | Out-Null
    $control.Writer.WriteLine('wait-for control ; display-message -p CONTROL-AFTER')
    Until $control '^%begin' | Out-Null
    No-Completion $control
    P wait-for -S control | Out-Null
    Until $control '^%end' | Out-Null
    $lines = Until $control '^%end'
    Check ($lines -contains 'CONTROL-AFTER') 'control list did not resume in order'

    P wait-for -L lost-locker | Out-Null
    $control.Writer.WriteLine('wait-for -L lost-locker')
    Until $control '^%begin' | Out-Null
    No-Completion $control
    $control.Socket.Close()
    $live = Start-CLI -Words @('wait-for', '-L', 'lost-locker')
    Pending $live
    P wait-for -U lost-locker | Out-Null
    Completed $live
    P wait-for -U lost-locker | Out-Null

    $persistent = Open-Raw PERSISTENT
    $persistent.Writer.WriteLine('wait-for persistent')
    $persistent.Writer.WriteLine('display-message -p PERSISTENT-AFTER')
    # Persistent display output is a popup in pushed JSON frames.
    No-Completion $persistent 'PERSISTENT-AFTER'
    P wait-for -S persistent | Out-Null
    Until $persistent 'PERSISTENT-AFTER' | Out-Null
    $persistent.Socket.Close()

    # The server really exits while both clients are pending. Prior output
    # must not substitute for the completion receipt on the command-list path.
    P set-option -g '@before-death' no | Out-Null
    $dying = Start-CLI -Words @('wait-for', 'server-death')
    $dyingChain = Start-CLI -Words @('display-message', '-p', 'BEFORE-DEATH', ';', 'set-option', '-g', '@before-death', 'yes', ';', 'wait-for', 'server-death')
    Pending $dying
    Pending $dyingChain
    # This command follows the display in the same list, so observing it
    # proves the display's output was written before the server is killed.
    Check ((P show-options -gv '@before-death') -ceq 'yes') 'command-list output did not precede the pending wait'
    P kill-server | Out-Null
    foreach ($child in @($dying, $dyingChain)) {
        $result = Finish $child
        Check ($result.Code -ne 0) 'server EOF falsely completed a pending request'
    }
    Write-Host "wait-for checks passed: $ns"
} finally {
    foreach ($socket in $sockets) { $socket.Close() }
    try {
        $cleanup = Finish (Start-CLI -Words @('kill-server'))
        Check ($cleanup.Code -eq 0 -or $cleanup.Err -match 'no server') "namespace cleanup failed: $($cleanup.Err)"
    } finally {
        try {
            foreach ($child in $children) {
                if (-not $child.Process.HasExited) { $child.Process.Kill() }
                $child.Process.WaitForExit()
            }
            Check ((Snapshot) -ceq $before) 'default namespace changed during wait-for checks'
        } finally {
            foreach ($child in $children) { $child.Process.Dispose() }
            Remove-Item -Recurse -Force $temp
        }
    }
}
