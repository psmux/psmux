param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\psmux.exe')
)

$ErrorActionPreference = 'Stop'
function Say { param([string]$Text) [Console]::Out.WriteLine($Text) }

if (-not (Test-Path -LiteralPath $Exe)) { Say "psmux binary not found: $Exe"; exit 4 }
$ExePath = (Resolve-Path -LiteralPath $Exe).Path
if ($env:PSMUX_DATA_DIR) { Say "PSMUX_DATA_DIR is already set, refusing: $env:PSMUX_DATA_DIR"; exit 4 }

$TempBase = [IO.Path]::GetTempPath()
$Root = Join-Path $TempBase "psmux-forward-limit-$PID"
Say "temp=$TempBase"
Say "root=$Root"
if (Test-Path -LiteralPath $Root) { Say "root already exists, refusing: $Root"; exit 4 }

$FixedNames = 'USERPROFILE', 'HOME', 'TEMP', 'TMP', 'TMUX', 'TMUX_PANE', 'XDG_CONFIG_HOME', 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_MODE', 'PSMUX_PSREADLINE_HISTORY', 'PSMUX_NO_WARM'
$PsmuxNames = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' } | ForEach-Object { $_.Name })
$Original = @{}
foreach ($name in @($FixedNames + $PsmuxNames | Select-Object -Unique)) {
    $Original[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$Namespace = "fcl$PID"
$RunId = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$FastLimitMs = 1000
$SlowMinMs = 1500
$SlowMaxMs = 5000
$RequestLimitMs = 10000
$Clock = [Diagnostics.Stopwatch]::StartNew()
$script:Codes = [Collections.Generic.List[int]]::new()
$script:Isolated = $false
$script:RootCreated = $false
$script:Held = [Collections.Generic.List[Net.Sockets.Socket]]::new()
$script:Listener = $null
$script:GhostFiles = @()
$script:Classes = @{}
$script:Busy = @{}
$BusyReply = "ERROR: no response from server (timed out)`n"

function Fail {
    param([int]$Code, [string]$Text)
    $script:Codes.Add($Code)
    Say "FAIL[$Code]: $Text"
}

function Pass { param([string]$Text) Say "PASS: $Text" }

function Show { param([string]$Text) ($Text -replace "`r", '<CR>' -replace "`n", '<LF>') }

function P {
    param([string[]]$Arguments)
    $info = [Diagnostics.ProcessStartInfo]::new($ExePath)
    foreach ($argument in @('-L', $Namespace) + $Arguments) { [void]$info.ArgumentList.Add($argument) }
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($info)
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($RequestLimitMs)) {
        try { $process.Kill() } catch {}
        return [pscustomobject]@{ Code = -1; Text = 'timeout' }
    }
    $text = ''
    if ($out.Wait(2000)) { $text += $out.Result }
    if ($err.Wait(500)) { $text += $err.Result }
    [pscustomobject]@{ Code = $process.ExitCode; Text = $text -replace '\r?\n$', '' }
}

function Wait-Until {
    param([scriptblock]$Condition, [int]$TimeoutMs = 5000)
    $deadline = $Clock.ElapsedMilliseconds + $TimeoutMs
    while ($Clock.ElapsedMilliseconds -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return [bool](& $Condition)
}

function Registry-Value {
    param([string]$Session, [string]$Extension)
    $path = Join-Path $env:PSMUX_DATA_DIR "$($Namespace)__$Session.$Extension"
    if (-not (Test-Path -LiteralPath $path)) { throw "no registry file $path" }
    [IO.File]::ReadAllText($path).Trim()
}

function Send-Line {
    param([int]$Port, [string]$Key, [string]$Line, [long]$Deadline)
    $started = $Clock.ElapsedMilliseconds
    $socket = [Net.Sockets.Socket]::new([Net.Sockets.AddressFamily]::InterNetwork, [Net.Sockets.SocketType]::Stream, [Net.Sockets.ProtocolType]::Tcp)
    $result = 'Error'
    $detail = ''
    $all = [IO.MemoryStream]::new()
    try {
        $remaining = $Deadline - $Clock.ElapsedMilliseconds
        if ($remaining -le 0) { throw [TimeoutException]::new('deadline passed before connect') }
        $connect = $socket.ConnectAsync([Net.IPAddress]::Loopback, $Port)
        if (-not $connect.Wait([int]$remaining)) { throw [TimeoutException]::new('connect') }
        $bytes = [Text.Encoding]::UTF8.GetBytes("AUTH $Key`n$Line`n")
        $sent = 0
        while ($sent -lt $bytes.Length) {
            $remaining = $Deadline - $Clock.ElapsedMilliseconds
            if ($remaining -le 0) { throw [TimeoutException]::new("send stopped after $sent of $($bytes.Length) bytes") }
            $socket.SendTimeout = [int]$remaining
            $count = $socket.Send($bytes, $sent, $bytes.Length - $sent, [Net.Sockets.SocketFlags]::None)
            if ($count -le 0) { throw [IO.IOException]::new("send returned $count after $sent of $($bytes.Length) bytes") }
            $sent += $count
        }
        $socket.Shutdown([Net.Sockets.SocketShutdown]::Send)
        $buffer = New-Object byte[] 8192
        while ($true) {
            $remaining = $Deadline - $Clock.ElapsedMilliseconds
            if ($remaining -le 0) { throw [TimeoutException]::new('read') }
            $socket.ReceiveTimeout = [int][Math]::Max(1, $remaining)
            $count = $socket.Receive($buffer)
            if ($count -eq 0) { break }
            $all.Write($buffer, 0, $count)
        }
        $result = 'Done'
    } catch [TimeoutException] {
        $result = 'Timeout'
        $detail = $_.Exception.Message
    } catch {
        $inner = $_.Exception
        while ($inner.InnerException) { $inner = $inner.InnerException }
        if ($inner -is [Net.Sockets.SocketException] -and $inner.SocketErrorCode -eq [Net.Sockets.SocketError]::TimedOut) {
            $result = 'Timeout'
        } else {
            $result = 'Error'
        }
        $detail = $inner.Message
    } finally {
        $socket.Dispose()
    }
    $elapsed = $Clock.ElapsedMilliseconds - $started
    $text = [Text.Encoding]::UTF8.GetString($all.ToArray())
    $body = ''
    if ($result -eq 'Done') {
        if ($text.StartsWith("OK`n")) { $body = $text.Substring(3) } else { $result = 'NoAuth'; $detail = $text }
    }
    [pscustomobject]@{ Result = $result; Ms = $elapsed; Body = $body; Detail = $detail }
}

function Request {
    param([string]$Session, [string]$Line, [long]$Deadline = -1, [switch]$WantBody)
    if ($Deadline -lt 0) { $Deadline = $Clock.ElapsedMilliseconds + $RequestLimitMs }
    $reply = Send-Line $script:Ports[$Session] $script:Keys[$Session] $Line $Deadline
    if ($reply.Result -ne 'Done') {
        Fail 2 "$Session [$Line] gave $($reply.Result) after $($reply.Ms) ms ($(Show $reply.Detail))"
        return $null
    }
    if ($reply.Body.StartsWith('ERROR')) {
        Fail 2 "$Session [$Line] gave an error body [$(Show $reply.Body)]"
        return $null
    }
    if (-not $WantBody -and $reply.Body.Length -ne 0) {
        Fail 2 "$Session [$Line] gave an unexpected body [$(Show $reply.Body)]"
        return $null
    }
    $reply
}

function Option-Value {
    param([string]$Session, [string]$Name, [long]$Deadline = -1)
    $reply = Request $Session "show-options -v $Name" $Deadline -WantBody
    if ($null -eq $reply) { return $null }
    $reply.Body -replace '\r?\n$', ''
}

function Set-Fresh {
    param([string]$Session, [string]$Name, [string]$Value)
    $set = Request $Session "set-option $Name $Value"
    if ($null -eq $set) { return $false }
    $got = Option-Value $Session $Name
    if ($got -cne $Value) { Fail 3 "$Session $Name is [$got] after it was set to [$Value]"; return $false }
    return $true
}

function Status-Message {
    param([string]$Session)
    $reply = Request $Session 'dump-state' -WantBody
    if ($null -eq $reply) { return $null }
    $match = [regex]::Match($reply.Body, '"status_message":"((?:[^"\\]|\\.)*)"')
    if ($match.Success) { return $match.Groups[1].Value }
    return ''
}

function Install-Hook {
    param([string]$Forward, [string]$Mark)
    $first = Request 'A' "set-hook -t A after-select-window `"$Forward`""
    $second = Request 'A' "set-hook -a -t A after-select-window `"$Mark`""
    if ($null -eq $first -or $null -eq $second) { return $false }
    $shown = Request 'A' 'show-hooks -t A' -WantBody
    if ($null -eq $shown) { return $false }
    $expected = "after-select-window[0] -> $Forward`nafter-select-window[1] -> $Mark"
    $got = $shown.Body -replace '(\r?\n)+$', ''
    if ($got -cne $expected) { Fail 3 "hooks of A are [$(Show $got)], expected [$(Show $expected)]"; return $false }
    Pass "hooks of A are [$(Show $got)]"
    return $true
}

function Fire-Hook {
    param([string]$Case, [string]$Window, [string]$Marker)
    $script:Busy[$Case] = 0
    $start = $Clock.ElapsedMilliseconds
    $deadline = $start + $RequestLimitMs
    $select = Request 'A' "select-window -t :$Window" $deadline
    if ($null -eq $select) { return $null }
    while ($true) {
        $reply = Send-Line $script:Ports['A'] $script:Keys['A'] 'show-options -v @hook_mark' $deadline
        if ($reply.Result -ne 'Done') {
            Fail 2 "A [show-options -v @hook_mark] gave $($reply.Result) after $($reply.Ms) ms ($(Show $reply.Detail))"
            return $null
        }
        if ($reply.Body -ceq $BusyReply) {
            $script:Busy[$Case]++
            Say "BUSY: $Case poll waited $($reply.Ms) ms and A answered [$(Show $reply.Body)]"
            continue
        }
        if ($reply.Body.StartsWith('ERROR')) {
            Fail 2 "A [show-options -v @hook_mark] gave an error body [$(Show $reply.Body)]"
            return $null
        }
        $value = $reply.Body -replace '\r?\n$', ''
        if ($value -ceq $Marker) { return $Clock.ElapsedMilliseconds - $start }
        if ($Clock.ElapsedMilliseconds -ge $deadline) { break }
        Start-Sleep -Milliseconds 20
    }
    Fail 3 "A @hook_mark did not become [$Marker] within $RequestLimitMs ms"
    return $null
}

function Wait-Value {
    param([string]$Session, [string]$Name, [string]$Value, [long]$Deadline)
    while ($true) {
        $got = Option-Value $Session $Name $Deadline
        if ($null -eq $got) { return $false }
        if ($got -ceq $Value) { return $true }
        if ($Clock.ElapsedMilliseconds -ge $Deadline) { break }
        Start-Sleep -Milliseconds 20
    }
    Fail 3 "$Session $Name did not become [$Value] before the deadline (last [$got])"
    return $false
}

function Classify-Live {
    param([string]$Case, $Ms)
    if ($null -eq $Ms) { return }
    $busy = [int]$script:Busy[$Case]
    if ($Ms -lt $FastLimitMs -and $busy -eq 0) { Pass "$Case took $Ms ms"; $script:Classes[$Case] = 'pass' }
    else { Fail 5 "$Case took $Ms ms with $busy busy replies, a live session must answer under $FastLimitMs ms with none" }
}

function Classify-Ghost {
    param([string]$Case, $Ms, [string]$Ghost)
    if ($null -eq $Ms) { return }
    $status = Status-Message 'A'
    if ($null -eq $status) { return }
    $expected = "can't connect to session: $Ghost"
    $busy = [int]$script:Busy[$Case]
    Say "CASE $Case ms=$Ms busy=$busy status=[$status]"
    if ($Ms -lt $FastLimitMs -and $busy -eq 0 -and $status -ceq $expected) {
        Pass "$Case took $Ms ms and A shows [$expected]"
        $script:Classes[$Case] = 'pass'
    } elseif ($Ms -ge $SlowMinMs -and $Ms -le $SlowMaxMs -and $status -notlike "can't connect to session*") {
        Say "SLOW: $Case took $Ms ms and A shows no connect message"
        $script:Classes[$Case] = 'slow'
    } else {
        Fail 5 "$Case took $Ms ms with status [$status] and $busy busy replies, which is neither pass (under $FastLimitMs ms, no busy reply, [$expected]) nor slow ($SlowMinMs to $SlowMaxMs ms without it)"
    }
}

function Test-Connect {
    param([int]$Port, [int]$WaitMs)
    $socket = [Net.Sockets.Socket]::new([Net.Sockets.AddressFamily]::InterNetwork, [Net.Sockets.SocketType]::Stream, [Net.Sockets.ProtocolType]::Tcp)
    $outcome = 'Error'
    $detail = ''
    try {
        $task = $socket.ConnectAsync([Net.IPAddress]::Loopback, $Port)
        if ($task.Wait($WaitMs)) {
            if ($socket.Connected) { return [pscustomobject]@{ Outcome = 'Connected'; Socket = $socket; Detail = '' } }
            $detail = 'the connect task ended without a connection'
        } else {
            $outcome = 'Full'
            $detail = "no connection within $WaitMs ms"
        }
    } catch {
        $inner = $_.Exception
        while ($inner.InnerException) { $inner = $inner.InnerException }
        $detail = $inner.Message
        $fullCodes = @([Net.Sockets.SocketError]::ConnectionRefused, [Net.Sockets.SocketError]::TimedOut)
        if ($inner -is [Net.Sockets.SocketException] -and $inner.SocketErrorCode -in $fullCodes) { $outcome = 'Full' }
    }
    $socket.Dispose()
    [pscustomobject]@{ Outcome = $outcome; Socket = $null; Detail = $detail }
}

function Saturated {
    param([int]$Port)
    $probe = Test-Connect $Port 3000
    if ($probe.Outcome -eq 'Full') { return $true }
    if ($probe.Outcome -eq 'Connected') {
        $script:Held.Add($probe.Socket)
        return $false
    }
    throw "the saturation check on port $Port failed: $($probe.Detail)"
}

function Test-PlainPath {
    param([string]$Path)
    $file = [IO.FileInfo]::new($Path)
    if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { return $false }
    for ($dir = $file.Directory; $null -ne $dir; $dir = $dir.Parent) {
        if ($dir.Attributes -band [IO.FileAttributes]::ReparsePoint) { return $false }
    }
    return $true
}

try {
    New-Item -ItemType Directory -Path $Root | Out-Null
    $script:RootCreated = $true
    foreach ($sub in 'data', 'home', 'tmp') { New-Item -ItemType Directory -Path (Join-Path $Root $sub) | Out-Null }
    $EmptyConfig = Join-Path $Root 'empty.conf'
    [IO.File]::WriteAllText($EmptyConfig, '')
    foreach ($name in $Original.Keys) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
    $env:PSMUX_DATA_DIR = Join-Path $Root 'data'
    $env:PSMUX_CONFIG_FILE = $EmptyConfig
    $env:PSMUX_PSREADLINE_HISTORY = Join-Path $Root 'history.txt'
    $env:PSMUX_NO_WARM = '1'
    $env:USERPROFILE = Join-Path $Root 'home'
    $env:HOME = Join-Path $Root 'home'
    $env:TEMP = Join-Path $Root 'tmp'
    $env:TMP = Join-Path $Root 'tmp'
    $leftover = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -notin 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_PSREADLINE_HISTORY', 'PSMUX_NO_WARM' })
    if ($leftover.Count -or -not (Test-Path -LiteralPath $env:PSMUX_DATA_DIR)) { throw "isolation is incomplete: $($leftover.Name -join ', ')" }
    $script:Isolated = $true

    foreach ($session in 'A', 'B') {
        $made = P @('new-session', '-d', '-s', $session, '-x', '120', '-y', '30')
        $up = Wait-Until { (P @('has-session', '-t', $session)).Code -eq 0 } 10000
        if ($made.Code -ne 0 -or -not $up) { throw "new-session $session failed (exit $($made.Code), $($made.Text))" }
    }
    $window = P @('new-window', '-d', '-t', 'A')
    if ($window.Code -ne 0) { throw "new-window in A failed (exit $($window.Code), $($window.Text))" }
    $script:Ports = @{}
    $script:Keys = @{}
    foreach ($session in 'A', 'B') {
        $script:Ports[$session] = [int](Registry-Value $session 'port')
        $script:Keys[$session] = Registry-Value $session 'key'
    }
    Say "run=$RunId A=$($script:Ports['A']) B=$($script:Ports['B'])"

    if (-not (Request 'A' 'set-option -g display-time 0')) { throw 'could not set display-time' }
    $displayTime = (Request 'A' 'show-options -gv display-time' -WantBody)
    if ($null -eq $displayTime -or ($displayTime.Body -replace '\r?\n$', '') -cne '0') { throw "display-time of A is not 0" }
    foreach ($name in '@fwd_probe', '@hook_probe') {
        if (-not (Set-Fresh 'A' $name "keep-$RunId")) { throw "could not set A $name" }
    }
    if (-not (Set-Fresh 'B' '@live_set' "before-$RunId")) { throw 'could not set B @live_set' }
    if (-not (Set-Fresh 'B' '@live_hook' "before-$RunId")) { throw 'could not set B @live_hook' }

    $value = "n1-$RunId"
    $start = $Clock.ElapsedMilliseconds
    $deadline = $start + $RequestLimitMs
    $n1 = Request 'A' "set-option -t B: @live_set $value" $deadline
    if ($null -ne $n1) {
        Say "CASE N1 ms=$($n1.Ms)"
        if (Wait-Value 'B' '@live_set' $value $deadline) { Classify-Live 'N1' $n1.Ms }
    }

    $value = "n2-$RunId"
    if ((Set-Fresh 'A' '@hook_mark' "reset-n2-$RunId") -and (Install-Hook "set -t B: @live_hook $value" "set @hook_mark $value")) {
        $start = $Clock.ElapsedMilliseconds
        $ms = Fire-Hook 'N2' '1' $value
        if ($null -ne $ms) {
            Say "CASE N2 ms=$ms"
            if (Wait-Value 'B' '@live_hook' $value ($start + $RequestLimitMs)) { Classify-Live 'N2' $ms }
        }
    }

    $before = Status-Message 'A'
    if ($null -ne $before -and $before -like "can't connect to session*") { Fail 3 "A already shows [$before] before the ghost cases" }

    $script:Listener = [Net.Sockets.Socket]::new([Net.Sockets.AddressFamily]::InterNetwork, [Net.Sockets.SocketType]::Stream, [Net.Sockets.ProtocolType]::Tcp)
    $script:Listener.Bind([Net.IPEndPoint]::new([Net.IPAddress]::Loopback, 0))
    $script:Listener.Listen(1)
    $GhostPort = $script:Listener.LocalEndPoint.Port
    for ($i = 0; $i -lt 64; $i++) {
        $connection = Test-Connect $GhostPort 500
        if ($connection.Outcome -eq 'Full') { break }
        if ($connection.Outcome -ne 'Connected') { throw "filling the ghost listener failed: $($connection.Detail)" }
        $script:Held.Add($connection.Socket)
    }
    Say "ghost port=$GhostPort held=$($script:Held.Count)"
    if (-not (Saturated $GhostPort)) { throw 'the ghost listener is not saturated before the ghost cases' }
    foreach ($ghost in 'ghost1', 'ghost2') {
        $path = Join-Path $env:PSMUX_DATA_DIR "$($Namespace)__$ghost.port"
        [IO.File]::WriteAllText($path, "$GhostPort")
        $script:GhostFiles += $path
    }

    $start = $Clock.ElapsedMilliseconds
    # The handler answers a forward it could not make with the error, as a
    # failed command does (1df45e7b); anything else in the body is a failure.
    $g1 = Send-Line $script:Ports['A'] $script:Keys['A'] "set-option -t ghost1: @fwd_probe v3-$RunId" ($start + $RequestLimitMs)
    if ($g1.Result -ne 'Done') {
        Fail 2 "A [set-option -t ghost1:] gave $($g1.Result) after $($g1.Ms) ms ($(Show $g1.Detail))"
    } elseif ($g1.Body.Length -ne 0 -and -not $g1.Body.StartsWith("ERROR: can't connect to session: ghost1")) {
        Fail 2 "A [set-option -t ghost1:] gave an unexpected body [$(Show $g1.Body)]"
    } else {
        Classify-Ghost 'G1' $g1.Ms 'ghost1'
    }

    $value = "g2-$RunId"
    if ((Set-Fresh 'A' '@hook_mark' "reset-g2-$RunId") -and (Install-Hook "set -t ghost2: @hook_probe v4-$RunId" "set @hook_mark $value")) {
        $ms = Fire-Hook 'G2' '0' $value
        Classify-Ghost 'G2' $ms 'ghost2'
    }

    foreach ($name in '@fwd_probe', '@hook_probe') {
        $got = Option-Value 'A' $name
        if ($got -cne "keep-$RunId") { Fail 3 "A $name is [$got], expected [keep-$RunId]" } else { Pass "A $name kept [keep-$RunId]" }
    }
    if (-not (Saturated $GhostPort)) { Fail 4 'the ghost listener was not saturated after the ghost cases' } else { Pass 'the ghost listener stayed saturated' }
    foreach ($path in $script:GhostFiles) {
        if (-not (Test-Path -LiteralPath $path)) { Fail 4 "ghost file was removed during the run: $path" }
    }
} catch {
    Fail 4 "script error $($_.Exception.Message)"
} finally {
    foreach ($path in $script:GhostFiles) {
        try {
            if (-not (Test-Path -LiteralPath $path)) { continue }
            if (-not (Test-PlainPath $path)) { Fail 4 "ghost file or its path holds a reparse point, not removed: $path"; continue }
            Remove-Item -LiteralPath $path -Force -ErrorAction Stop
        } catch {
            Fail 4 "could not check or remove $path ($($_.Exception.Message))"
        }
    }
    foreach ($socket in $script:Held) { $socket.Dispose() }
    if ($null -ne $script:Listener) { $script:Listener.Dispose() }
    if ($script:Isolated) {
        [void](P @('kill-server'))
        $gone = Wait-Until { @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $ExePath }).Count -eq 0 } 10000
        if (-not $gone) { Fail 4 'a process of this binary is left after kill-server' }
    }
    foreach ($name in $Original.Keys) {
        if ($null -eq $Original[$name]) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $Original[$name], 'Process') }
    }
    if ($script:RootCreated) {
        try {
            $links = @(Get-ChildItem -LiteralPath $Root -Recurse -Force -Attributes ReparsePoint -ErrorAction Stop)
            for ($dir = [IO.DirectoryInfo]::new($Root); $null -ne $dir; $dir = $dir.Parent) {
                if ($dir.Attributes -band [IO.FileAttributes]::ReparsePoint) { $links += $dir }
            }
            if ($links.Count) { throw "root, its path or its contents hold reparse points: $($links.FullName -join ', ')" }
            Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction Stop
        } catch {
            Fail 4 "could not remove $Root ($($_.Exception.Message))"
        }
    }
}

$classText = ($script:Classes.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join ' '
Say "classes: $classText"
if ($script:Codes.Count -gt 0) {
    $worst = ($script:Codes | Measure-Object -Maximum).Maximum
    Say "$($script:Codes.Count) check(s) failed, exit $worst"
    exit $worst
}
$ghosts = @($script:Classes['G1'], $script:Classes['G2'])
$lives = @($script:Classes['N1'], $script:Classes['N2'])
if (($lives | Where-Object { $_ -ne 'pass' }).Count -gt 0) { Say 'a live case has no result, exit 5'; exit 5 }
if (($ghosts | Where-Object { $_ -eq 'pass' }).Count -eq 2) { Say 'all cases pass, exit 0'; exit 0 }
if (($ghosts | Where-Object { $_ -eq 'slow' }).Count -eq 2) { Say 'both ghost cases were slow without a connect message, exit 1'; exit 1 }
Say "the ghost cases disagree or have no result ($($ghosts -join ', ')), exit 5"
exit 5
