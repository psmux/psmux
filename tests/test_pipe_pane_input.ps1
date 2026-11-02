#requires -Version 7.0
# Real pipe-pane input/output with owned, gated producers and raw console input.
# PSMUX_EXE overrides the normal release/debug discovery. No special test gate.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\psmux_test_helpers.ps1"
$exe = Get-PsmuxExe
function Default-Sessions {
    $text = & $exe list-sessions -F '#{session_id}:#{session_name}' 2>&1 | Out-String
    $code = $LASTEXITCODE
    if ($code -ne 0 -and $text -notmatch 'no server|no sessions|failed to connect|cannot connect') {
        throw "default session snapshot failed ($code): $text"
    }
    return "$code`n$text"
}
$before = Default-Sessions
$savedServerMode = $env:PSMUX_SERVER_MODE
$ctx = New-PsmuxTestEnv -Tag 'pipeinput' -Exe $exe
$owned = [Collections.Generic.List[Diagnostics.Process]]::new()
try {
    $env:PSMUX_SERVER_MODE = 'per-session'
    $ns = Register-PsmuxNamespace -Ctx $ctx -Namespace ('pipeinput-' + [guid]::NewGuid().ToString('N'))
    $work = Join-Path $ctx.Home 'pipe-input'
    New-Item -ItemType Directory $work | Out-Null
    $probe = Join-Path $work 'probe.ps1'
    Copy-Item "$PSScriptRoot\pipe_pane_probe.cs" $work
    @'
param($Mode, $Root, [int]$Total)
$ErrorActionPreference = 'Stop'
try {
    Add-Type -Path (Join-Path $PSScriptRoot 'pipe_pane_probe.cs')
    [PipePaneProbe]::Run($Mode, $Root, $Total)
    exit 0
} catch {
    [Console]::Error.WriteLine($_)
    exit 1
}
'@ | Set-Content $probe
    function Assert([bool]$Condition, [string]$Message) {
        if (-not $Condition) { throw $Message }
        Write-Host "PASS: $Message"
    }
    function Wait-For([scriptblock]$Condition, [string]$Message, [int]$TimeoutSeconds = 60) {
        $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        while (-not (& $Condition)) {
            if ([DateTime]::UtcNow -ge $deadline) { throw "timeout: $Message" }
            Start-Sleep -Milliseconds 20
        }
    }
    function Read-Bytes([string]$Path) {
        if (-not (Test-Path $Path)) { return ,([byte[]]::new(0)) }
        $file = [IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
        try {
            $memory = [IO.MemoryStream]::new()
            try { $file.CopyTo($memory); return ,$memory.ToArray() } finally { $memory.Dispose() }
        } finally { $file.Dispose() }
    }
    function Events([string]$Root) { return [Text.Encoding]::ASCII.GetString((Read-Bytes "$Root.events")) }
    function Has([string]$Root, [string]$Event) { return (Events $Root).Split("`n").Contains($Event) }
    function Touch([string]$Path) { [IO.File]::WriteAllText($Path, 'go') }
    function Quote([string]$Text) { return "'" + $Text.Replace("'", "''") + "'" }
    function Command([string]$Mode, [string]$Root, [int]$Total = 0) {
        # Dot-source into the owned sink shell; its PID owns the native I/O.
        return '. ' + (Quote $probe) + ' ' + (Quote $Mode) + ' ' + (Quote $Root) + " $Total"
    }
    function P {
        $start = [Diagnostics.ProcessStartInfo]::new($exe)
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($arg in @('-L', $ns) + $args) { $start.ArgumentList.Add([string]$arg) }
        $client = [Diagnostics.Process]::Start($start)
        try {
            $stdout = $client.StandardOutput.ReadToEndAsync()
            $stderr = $client.StandardError.ReadToEndAsync()
            if (-not $client.WaitForExit(5000)) {
                $client.Kill(); [void]$client.WaitForExit(5000)
                throw "server command exceeded five seconds: $args"
            }
            $text = $stdout.GetAwaiter().GetResult()
            $errors = $stderr.GetAwaiter().GetResult()
            if ($client.ExitCode -ne 0) { throw "psmux exit $($client.ExitCode): $args : $errors $text" }
            return $text.Trim()
        } finally { $client.Dispose() }
    }
    function Own([string]$Root) {
        Wait-For { Has $Root 'ready' } "$Root starts"
        $process = [Diagnostics.Process]::GetProcessById([int][IO.File]::ReadAllText("$Root.pid"))
        $null = $process.SafeHandle # Keep identity even if the numeric PID is reused.
        $owned.Add($process)
        return $process
    }
    function Pane([string]$Name, [string]$Mode, [int]$Total) {
        $root = Join-Path $work $Name
        P new-window -d -t s -n $Name (Command $Mode $root $Total) | Out-Null
        $null = Own $root
        if ($Mode -eq 'receive') { Wait-For { Has $root 'raw' } 'raw input configured' }
        return $root
    }
    function Pipe([string]$Target, [string]$Mode, [string]$Root, [int]$Total, [string[]]$Flags) {
        P pipe-pane @Flags -t "s:$Target" (Command $Mode $Root $Total) | Out-Null
        $process = Own $Root
        # This reply is a server-queue barrier after the pipe request.
        Assert ((P display-message -t s:0 -p 'PIPE_BARRIER') -eq 'PIPE_BARRIER') 'server answers installation barrier'
        return $process
    }
    function Check-Input([string]$Root, [int]$Total, [int]$TimeoutSeconds = 60) {
        Wait-For { Has $Root 'received-all' } 'real pane consumed the complete input' $TimeoutSeconds
        $bytes = Read-Bytes "$Root.bytes"
        Assert ($bytes.Length -eq $Total) 'PTY receiver captured the exact byte count'
        for ($i = 0; $i -lt $Total; $i++) {
            if ($bytes[$i] -ne (97 + $i % 26)) { throw "PTY input mismatch at $i" }
        }
        Write-Host 'PASS: actual raw-console input is byte-exact and ordered (ASCII payload)'
    }
    function Progress([string]$Root) {
        $matches = [regex]::Matches((Events $Root), '(?m)^wrote (\d+)$')
        if ($matches.Count -eq 0) { return 0 }
        return [int]$matches[$matches.Count - 1].Groups[1].Value
    }
    function Stalled([string]$Root, [int]$Total) {
        Wait-For { (Progress $Root) -gt 0 } 'producer completed actual stdout writes'
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        $stableSince = [DateTime]::UtcNow
        $last = Progress $Root
        do {
            Assert ((P display-message -t s:0 -p 'STALLED_PROBE') -eq 'STALLED_PROBE') 'unrelated server command answers during backpressure'
            Start-Sleep -Milliseconds 100
            $now = Progress $Root
            if ($now -ne $last) { $stableSince = [DateTime]::UtcNow; $last = $now }
            if ($last -ge $Total) { throw 'producer exhausted its payload instead of being backpressured' }
            if ([DateTime]::UtcNow -ge $deadline) { throw 'producer never reached stable backpressure' }
        } while (([DateTime]::UtcNow - $stableSince).TotalSeconds -lt 1)
        Assert ($last -gt 0 -and $last -lt $Total) "producer stalled after $last actual bytes, before $Total"
    }

    P new-session -d -s s 'pwsh -NoProfile' | Out-Null
    $total = 8 * 1024 * 1024
    # Small immediate-exit source: upstream RED for -I, and drain-before-reap.
    $root = Pane 'input' 'receive' 128
    $source = Join-Path $work 'source'
    $child = Pipe 'input' 'produce' $source 128 @('-I')
    Touch "$root.go"; Touch "$source.go"
    Check-Input $root 128
    Wait-For { Has $source 'exit 0' } 'source succeeds before retirement'
    Assert ($child.WaitForExit(5000)) 'EOF source exits'

    # EOF must retire the old input pipe so -o re-arms rather than toggling off.
    $rearm = Join-Path $work 'rearm'
    $child = Pipe 'input' 'produce' $rearm 32 @('-I', '-o')
    Touch "$rearm.go"
    Wait-For { Has $rearm 'exit 0' } '-o launches a new source after EOF'
    Assert ($child.WaitForExit(5000)) 're-armed source exits'

    # Closing stdout does not cancel the command's remaining work.
    $root = Pane 'stdout-eof' 'receive' 128
    $source = Join-Path $work 'stdout-eof-source'
    $child = Pipe 'stdout-eof' 'produce-close' $source 128 @('-I')
    Touch "$root.go"; Touch "$source.go"
    Wait-For { (Has $source 'stdout-closed') -or $child.HasExited } 'source closes stdout'
    Assert (Has $source 'stdout-closed') 'source reaches its post-stdout gate'
    Check-Input $root 128
    P display-message -t s:0 -p 'SOURCE_EOF_BARRIER' | Out-Null
    P display-message -t s:0 -p 'SOURCE_EOF_BARRIER' | Out-Null
    Assert (-not $child.HasExited) 'stdout EOF does not terminate the command'
    Touch "$source.finish"
    Wait-For { (Has $source 'exit 0') -or $child.HasExited } 'command finishes after stdout EOF'
    Assert ((Has $source 'exit 0') -and $child.WaitForExit(5000) -and $child.ExitCode -eq 0) 'post-stdout work finishes successfully'

    $root = Pane 'bounded' 'receive' $total
    $source = Join-Path $work 'bounded-source'
    $child = Pipe 'bounded' 'produce' $source $total @('-I')
    Assert (-not (Test-Path "$root.bytes")) 'nonreader has not opened its capture'
    Touch "$source.go"
    # ConPTY may buffer input while its application is not reading. The gated
    # writer tests check queue backpressure; here check real bulk delivery.
    Wait-For { (Progress $source) -gt 0 } 'source completes actual stdout writes'
    Assert ((P display-message -t s:0 -p 'INPUT_PROBE') -eq 'INPUT_PROBE') 'server answers while the pane application is not reading'
    Touch "$root.go"
    # Detached input follows the server's idle poll cadence.
    Check-Input $root $total 120
    Wait-For { Has $source "produced $total" } 'input producer completes its payload'
    Assert ($child.WaitForExit(5000)) 'bulk input source exits'

    # Close/toggle/replace while an owned input source waits at its write gate.
    foreach ($action in @('close', 'toggle', 'replace', 'kill-pane')) {
        $root = Pane $action 'receive' $total
        $source = Join-Path $work "$action-source"
        $child = Pipe $action 'produce' $source $total @('-I')
        Assert (-not $child.HasExited) 'input source is alive before its write gate opens'
        if ($action -eq 'close') { P pipe-pane -t "s:$action" | Out-Null }
        elseif ($action -eq 'toggle') {
            P pipe-pane -I -o -t "s:$action" (Command 'fail' (Join-Path $work 'must-not-start')) | Out-Null
        } elseif ($action -eq 'kill-pane') {
            P kill-pane -t "s:$action" | Out-Null
        } else {
            $replacement = Join-Path $work 'replacement'
            $new = Pipe $action 'produce' $replacement 128 @('-I')
            P pipe-pane -t "s:$action" | Out-Null
            Assert ($new.WaitForExit(5000)) 'replacement source stops without waiting for pane input'
        }
        Assert ($child.WaitForExit(5000)) "$action stops the active input source"
        Assert (-not (Test-Path (Join-Path $work 'must-not-start.pid'))) '-o does not start a replacement'
    }

    # Both directions use one pipe; receiver reply comes from real pane output.
    $root = Pane 'duplex' 'receive' 128
    Touch "$root.reply"; Touch "$root.go"
    $sink = Join-Path $work 'duplex-sink'
    $child = Pipe 'duplex' 'duplex' $sink 128 @('-IO')
    Touch "$sink.go"
    Check-Input $root 128
    Wait-For { Has $sink 'eof' } 'duplex child receives the pane reply'
    Assert ([Text.Encoding]::ASCII.GetString((Read-Bytes "$sink.bytes")).Contains('PANE_REPLY_TOKEN')) '-IO routes actual pane output back to child stdin'
    Assert ($child.WaitForExit(5000)) 'duplex command exits'

    # Default/-O and ordinary PTY EOF retain output tails. Capture raw output,
    # not an echo-derived approximation of what was written to pane stdin.
    foreach ($flag in @('default', '-O')) {
        $name = if ($flag -eq 'default') { 'default-output' } else { 'explicit-output' }
        $root = Pane $name 'output-exit' 32768
        if ($flag -eq '-O') { P set-window-option -t "s:$name" remain-on-exit on | Out-Null }
        $sink = Join-Path $work "$name-sink"
        $flags = @()
        if ($flag -eq '-O') { $flags = @('-O') }
        $child = Pipe $name 'sink' $sink 0 $flags
        Assert ((P display-message -t "s:$name" -p '#{pane_dead}') -eq '0') 'output pane starts alive'
        Touch "$root.go"
        if ($flag -eq '-O') {
            Wait-For { (P display-message -t "s:$name" -p '#{pane_dead}') -eq '1' } 'output pane is retained after process exit'
            Wait-For { [Text.Encoding]::ASCII.GetString((Read-Bytes "$sink.bytes")).Contains('PIPE_TAIL_TOKEN') } 'retained pane delivers its output tail'
            P pipe-pane -t "s:$name" | Out-Null
        } else {
            Wait-For { Has $sink 'eof' } 'PTY reader EOF closes child stdin after output drain'
        }
        Assert ([Text.Encoding]::ASCII.GetString((Read-Bytes "$sink.bytes")).Contains('PIPE_TAIL_TOKEN')) "$name preserves the output tail"
        Assert ($child.WaitForExit(5000)) 'output sink exits after EOF or explicit close'
    }

    foreach ($action in @('resume', 'stop')) {
        $root = Pane "output-$action" 'output' $total
        $sink = Join-Path $work "output-$action-sink"
        Touch "$sink.stall"
        $child = Pipe "output-$action" 'sink' $sink 0 @('-O')
        Touch "$root.go"; Stalled $root $total
        Assert (-not $child.HasExited) 'nonreading output sink stays alive during backpressure'
        if ($action -eq 'resume') {
            Touch "$sink.go"
            Wait-For { Has $root "produced $total" } 'pane output resumes after sink starts reading'
            Wait-For { [Text.Encoding]::ASCII.GetString((Read-Bytes "$sink.bytes")).Contains('PIPE_TAIL_TOKEN') } 'resumed sink receives final output marker'
        }
        P pipe-pane -t "s:output-$action" | Out-Null
        Assert ($child.WaitForExit(5000)) 'stopping a stalled/resumed sink is responsive'
        Wait-For { Has $root "produced $total" } 'removing the stalled sink lets pane output finish'
    }

    # Nonzero child exit must not leave a phantom pipe that -o toggles off.
    $root = Pane 'failure-target' 'receive' 128
    Touch "$root.go"
    $failed = Join-Path $work 'failure'
    $child = Pipe 'failure-target' 'fail' $failed 0 @('-I')
    Touch "$failed.go"
    Wait-For { (Events $failed).Contains('exit 1 IOException') } 'child failure is observed'
    Assert ($child.WaitForExit(5000) -and $child.ExitCode -ne 0) 'pipe child exits nonzero'
    $afterFailure = Join-Path $work 'after-failure'
    $child = Pipe 'failure-target' 'produce' $afterFailure 128 @('-I', '-o')
    Touch "$afterFailure.go"
    Check-Input $root 128
    Wait-For { Has $afterFailure 'exit 0' } '-o re-arms after child failure'
    Assert ($child.WaitForExit(5000)) 'replacement after failure exits'
    $shutdown = Join-Path $work 'shutdown-source'
    $child = Pipe 'failure-target' 'produce' $shutdown 128 @('-I')
    P kill-server | Out-Null
    Assert ($child.WaitForExit(5000)) 'namespace shutdown stops the owned, quiet input source'
    Write-Host 'All pipe-pane input/output runtime checks passed.'
} catch {
    $failure = $_
    try {
        foreach ($file in Get-ChildItem -LiteralPath $work -File | Where-Object Extension -in '.events', '.bytes') {
            $stream = [IO.File]::Open($file.FullName, 'Open', 'Read', 'ReadWrite')
            try {
                if ($file.Extension -eq '.bytes') { Write-Host "EVIDENCE $($file.Name): $($stream.Length) captured bytes"; continue }
                [void]$stream.Seek([Math]::Max(0, $stream.Length - 1024), 'Begin')
                $reader = [IO.StreamReader]::new($stream)
                try { Write-Host "EVIDENCE $($file.Name): $($reader.ReadToEnd())" } finally { $reader.Dispose() }
            } finally { $stream.Dispose() }
        }
    } catch { Write-Host "Failure evidence unavailable: $_" }
    throw $failure
} finally {
    # Retained Process handles identify only children created by this fixture.
    # Release them first so a blocked tee cannot wedge cleanup.
    $cleanupErrors = [Collections.Generic.List[string]]::new()
    foreach ($process in $owned) {
        try {
            if (-not $process.HasExited) {
                $process.Kill()
                if (-not $process.WaitForExit(5000)) { $cleanupErrors.Add("owned child $($process.Id) did not exit") }
            }
        } catch { $cleanupErrors.Add($_.ToString()) }
        finally { $process.Dispose() }
    }
    try { Remove-PsmuxTestEnv -Ctx $ctx }
    finally {
        $env:PSMUX_SERVER_MODE = $savedServerMode
        $after = Default-Sessions
        if ($before -cne $after) { throw "default sessions changed: BEFORE=$before AFTER=$after" }
    }
    if ($cleanupErrors.Count) { throw "owned child cleanup failed: $($cleanupErrors -join '; ')" }
}
