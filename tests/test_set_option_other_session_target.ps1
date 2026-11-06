param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\psmux.exe')
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Exe)) { Write-Output "psmux binary not found: $Exe"; exit 2 }
$ExePath = (Resolve-Path -LiteralPath $Exe).Path

$TempBase = [IO.Path]::GetTempPath()
$Root = Join-Path $TempBase "psmux-set-forward-$PID"
Write-Output "temp=$TempBase"
Write-Output "root=$Root"
if (Test-Path -LiteralPath $Root) { Write-Output "root already exists, refusing: $Root"; exit 2 }

$FixedNames = 'USERPROFILE', 'HOME', 'TEMP', 'TMP', 'TMUX', 'TMUX_PANE', 'XDG_CONFIG_HOME', 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_MODE', 'PSMUX_PSREADLINE_HISTORY', 'PSMUX_NO_WARM'
$PsmuxNames = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' } | ForEach-Object { $_.Name })
$Original = @{}
foreach ($name in @($FixedNames + $PsmuxNames | Select-Object -Unique)) {
    $Original[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$Namespace = "sof$PID"
$CR = [string][char]13
$script:Failures = 0
$script:Isolated = $false
$script:RootCreated = $false
$script:Sessions = @('A', 'B')

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { Write-Output "PASS: $Name" } else { $script:Failures++; Write-Output "FAIL: $Name $Detail" }
}

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
    if (-not $process.WaitForExit(10000)) {
        try { $process.Kill() } catch {}
        $script:Failures++
        [Console]::Out.WriteLine("FAIL: timeout after 10s: $($Arguments -join ' ')")
        return [pscustomobject]@{ Code = -1; Text = '' }
    }
    $text = ''
    if ($out.Wait(2000)) { $text += $out.Result }
    if ($err.Wait(500)) { $text += $err.Result }
    [pscustomobject]@{ Code = $process.ExitCode; Text = $text -replace '\r?\n$', '' }
}

function Wait-Until {
    param([scriptblock]$Condition, [int]$TimeoutMs = 5000)
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return [bool](& $Condition)
}

function Hex { param([string]$Text) ($Text.ToCharArray() | ForEach-Object { '{0:X2}' -f [int]$_ }) -join ' ' }

function Shown { param([string]$Text) $Text -replace "`r", '<CR>' -replace "`n", '<LF>' }

function Read-Option {
    param([string[]]$Arguments)
    $result = P (@('show-options', '-v') + $Arguments)
    if ($result.Code -ne 0 -and $result.Text -match 'invalid option') { return $null }
    if ($result.Code -ne 0) { throw "show-options $($Arguments -join ' ') failed: $($result.Text)" }
    $result.Text
}

function Value { param([string]$Session, [string]$Name = '@probe') Read-Option @('-t', $Session, $Name) }

function Global-Value { param([string]$Session, [string]$Name = '@probe') Read-Option @('-g', '-t', $Session, $Name) }

function Shown-Value { param($Value) if ($null -eq $Value) { 'none' } else { "[$(Shown $Value)]" } }

function Registry-Value {
    param([string]$Session, [string]$Extension)
    $file = Get-ChildItem -LiteralPath $env:PSMUX_DATA_DIR -Recurse -File -Filter "$($Namespace)__$Session.$Extension" | Select-Object -First 1
    if ($null -eq $file) { throw "no registry file $($Namespace)__$Session.$Extension" }
    [IO.File]::ReadAllText($file.FullName).Trim()
}

function Send-Lines {
    param([string]$Session, [string[]]$Lines, [int]$ReadMs = 1000)
    $port = [int](Registry-Value $Session 'port')
    $key = Registry-Value $Session 'key'
    $client = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    try {
        $stream = $client.GetStream()
        $payload = "AUTH $key`n" + (($Lines | ForEach-Object { "$_`n" }) -join '')
        $bytes = [Text.Encoding]::UTF8.GetBytes($payload)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
        $stream.ReadTimeout = $ReadMs
        $buffer = New-Object byte[] 65536
        $all = [IO.MemoryStream]::new()
        try { while (($count = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) { $all.Write($buffer, 0, $count) } } catch {}
        [Text.Encoding]::UTF8.GetString($all.ToArray())
    } finally {
        $client.Close()
    }
}

function One-Shot { param([string]$Line) Send-Lines 'A' @($Line) }

function Persistent { param([string]$Line) [void](Send-Lines 'A' @('PERSISTENT', $Line) 1500) }

function Status-Message {
    param([string]$Session)
    $state = Send-Lines $Session @('PERSISTENT', 'dump-state') 1500
    if ($state -match '"status_message":"((?:[^"\\]|\\.)*)"') { return $Matches[1] }
    $null
}

function Reset-Values {
    foreach ($session in $script:Sessions) {
        foreach ($name in '@probe', '@probe2', '@other') {
            [void](P @('set-option', '-u', '-t', $session, $name))
            [void](P @('set-option', '-gu', '-t', $session, $name))
        }
    }
    foreach ($session in $script:Sessions) {
        foreach ($name in '@probe', '@probe2', '@other') {
            if ($null -ne (Value $session $name) -or $null -ne (Global-Value $session $name)) { throw "could not reset $name in $session" }
        }
    }
}

function Check-Values {
    param([string]$Row, [hashtable]$Expected)
    foreach ($entry in $Expected.GetEnumerator()) {
        $session, $name = $entry.Key -split '\.', 2
        if (-not $name) { $name = '@probe' }
        $actual = Value $session $name
        Check ($actual -ceq $entry.Value) "$Row $session $name is $(Shown-Value $entry.Value)" "(got $(Shown-Value $actual))"
    }
}

function Check-Hex {
    param([string]$Row, [string]$Session, [string]$Name, [string]$ExpectedHex)
    $actual = Value $Session $Name
    $hex = if ($null -eq $actual) { 'none' } else { Hex $actual }
    Check ($hex -ceq $ExpectedHex) "$Row $Session $Name hex is [$ExpectedHex]" "(got [$hex])"
}

function Start-Sessions {
    param([string[]]$Names)
    foreach ($session in $Names) {
        $made = P @('new-session', '-d', '-s', $session, '-x', '120', '-y', '30')
        $up = Wait-Until { (P @('has-session', '-t', $session)).Code -eq 0 } 10000
        Check ($made.Code -eq 0 -and $up) "new-session $session is up" "(new-session exit $($made.Code))"
    }
}

function Stop-Server {
    [void](P @('kill-server'))
    $gone = Wait-Until { @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $ExePath }).Count -eq 0 } 10000
    Check $gone "no process of this binary is left after kill-server"
}

function Server-Threads {
    $process = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $ExePath })
    ($process | ForEach-Object { $_.Threads.Count } | Measure-Object -Sum).Sum
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

    $script:Sessions = @('A', 'B', 'Charlie')
    Start-Sessions $script:Sessions
    [void](P @('set-option', '-g', '-t', 'A', 'display-time', '0'))
    $BId = (P @('display-message', '-p', '-t', 'B', '#{session_id}')).Text
    Check ($BId -match '^\$\d+$') "B has a session id" "(got [$BId])"

    Reset-Values
    $reply = One-Shot 'set -t B: @probe p1'
    Check-Values 'r1' @{ 'A' = $null; 'B' = 'p1' }

    Reset-Values
    Persistent 'set -t B: @probe p2'
    Check-Values 'r2' @{ 'A' = $null; 'B' = 'p2' }

    Reset-Values
    Persistent 'set -t A: @probe p3'
    Check-Values 'r3' @{ 'A' = 'p3'; 'B' = $null }

    Reset-Values
    Persistent 'set -t =B: @probe p4'
    Check-Values 'r4' @{ 'A' = $null; 'B' = 'p4' }
    Reset-Values
    Persistent "set -t '$BId' @probe p4"
    Check-Values 'r4' @{ 'A' = $null; 'B' = 'p4' }
    Reset-Values
    Persistent 'set -t Cha: @probe p4'
    Check-Values 'r4' @{ 'A' = $null; 'Charlie' = 'p4' }

    Reset-Values
    Persistent 'set -t Nope: @probe p5'
    Check-Values 'r5' @{ 'A' = $null; 'B' = $null }
    $status = Status-Message 'A'
    Check ($null -ne $status -and $status -like "*can't find session: Nope*") "r5 A status message names Nope" "(got $(Shown-Value $status))"

    Reset-Values
    Persistent 'set @probe p6 -t B:'
    Check-Values 'r6' @{ 'A' = $null; 'B' = 'p6' }

    Reset-Values
    Persistent 'set -t B: -t A: @probe p7'
    Check-Values 'r7' @{ 'A' = 'p7'; 'B' = $null }
    Reset-Values
    Persistent 'set -t A: -t B: @probe p8'
    Check-Values 'r7' @{ 'A' = $null; 'B' = 'p8' }

    Reset-Values
    Persistent 'set -t B: @probe p9 ; set -t A: @probe p10'
    Check-Values 'r8' @{ 'A' = 'p10'; 'B' = 'p9' }

    Reset-Values
    Persistent "set -t B: @probe x ';' set @other v"
    Check-Values 'r9' @{ 'A' = $null; 'A.@other' = $null; 'B' = 'x ; set @other v'; 'B.@other' = $null }

    Reset-Values
    Persistent 'set -t B: -- @probe -t'
    Check-Values 'r10' @{ 'A' = $null; 'B' = '-t' }

    Reset-Values
    Persistent 'set -g -t B: @probe p11'
    $globalA = Global-Value 'A'
    $globalB = Global-Value 'B'
    Check ($null -eq $globalA) "r11 A global @probe is none" "(got $(Shown-Value $globalA))"
    Check ($globalB -ceq 'p11') "r11 B global @probe is [p11]" "(got $(Shown-Value $globalB))"

    Reset-Values
    $cli = P @('set-option', '-t', 'B', '@probe', 'p12')
    Check ($cli.Code -eq 0) "r12 CLI set-option exits 0" "(exit $($cli.Code) [$($cli.Text)])"
    Check-Values 'r12' @{ 'A' = $null; 'B' = 'p12' }

    Reset-Values
    Persistent "set -t B: @probe '\\;' set @other v"
    Check-Values 'r14' @{ 'A' = $null; 'A.@other' = $null; 'B' = '\\; set @other v'; 'B.@other' = $null }

    Reset-Values
    Persistent "set -t =B: @probe '\\;' set @other v"
    Check-Values 'r15' @{ 'A' = $null; 'A.@other' = $null; 'B' = '\\; set @other v'; 'B.@other' = $null }

    Reset-Values
    [void](P @('display-message', '-t', 'A', 'mark16'))
    $before16 = Status-Message 'A'
    Check ($before16 -ceq 'mark16') "r16 A status message is the mark before the line" "(got $(Shown-Value $before16))"
    $reply = One-Shot ('set -t B: @probe "a' + $CR + 'b"')
    Check-Hex 'r16' 'A' '@probe' '61 0D 62'
    Check-Hex 'r16' 'B' '@probe' 'none'
    $after16 = Status-Message 'A'
    Check ($after16 -ceq 'mark16') "r16 A status message is still the mark" "(got $(Shown-Value $after16))"
    Check ($reply -notmatch 'ERROR') "r16 A replies without an error" "(reply [$(Shown $reply)])"

    Reset-Values
    Persistent ('set -t =B: @probe "a' + $CR + 'b"')
    Check-Hex 'r18' 'A' '@probe' '61 0D 62'
    Check-Hex 'r18' 'B' '@probe' 'none'
    Persistent ('set -t A: @probe2 "a' + $CR + 'b"')
    Check-Hex 'r18' 'A' '@probe2' '61 0D 62'

    Reset-Values
    $alias = Send-Lines 'A' @('CONTROL_QUIET', 'set -s command-alias[0] "pb=set -t B: @probe ''a\nb''"') 2000
    $reply = One-Shot 'pb'
    Check-Hex 'r19' 'A' '@probe' '61 0A 62'
    Check-Hex 'r19' 'B' '@probe' 'none'
    Reset-Values
    Check (($null -eq (Value 'A')) -and ($null -eq (Value 'B'))) "r19 A and B @probe are none before the persistent call"
    Persistent 'pb'
    Check-Hex 'r19' 'A' '@probe' '61 0A 62'
    Check-Hex 'r19' 'B' '@probe' 'none'

    Reset-Values
    $reply = One-Shot ('set -t "B:' + $CR + '" @probe2 t1')
    Check ($reply -cmatch ("ERROR: can't find window: " + $CR)) "r20 A replies can't find window with the CR" "(reply [$(Shown $reply)])"
    Check-Values 'r20' @{ 'A.@probe2' = $null; 'B.@probe2' = $null }

    Start-Sessions @('1')
    $script:Sessions = @('A', 'B', 'Charlie', '1')
    Reset-Values
    Persistent 'set -t 1 @probe p17'
    Check-Values 'r17' @{ '1' = 'p17'; 'A' = $null; 'B' = $null }
    $window = P @('new-window', '-d', '-t', 'A:1')
    Check ($window.Code -eq 0) "r17 A has a window 1" "(exit $($window.Code) [$($window.Text)])"
    Persistent 'set -t 1 @probe p18'
    $windowValue = Read-Option @('-w', '-t', 'A:1', '@probe')
    Check ($null -eq $windowValue) "r17 A:1 window @probe is none" "(got $(Shown-Value $windowValue))"
    Check-Values 'r17' @{ '1' = 'p17'; 'A' = 'p18'; 'B' = $null }

    Stop-Server
    $script:Sessions = @('A', 'B')
    $env:PSMUX_SERVER_MODE = 'shared'
    Start-Sessions @('A', 'B')
    foreach ($session in 'A', 'B') { [void](P @('set-option', '-g', '-t', $session, 'display-time', '0')) }
    Reset-Values
    foreach ($session in 'A', 'B') { [void](P @('display-message', '-t', $session, "mark13$session")) }
    $markA13 = Status-Message 'A'
    $markB13 = Status-Message 'B'
    Check ($markA13 -ceq 'mark13A') "r13 shared A status message is the mark before the line" "(got $(Shown-Value $markA13))"
    Check ($markB13 -ceq 'mark13B') "r13 shared B status message is the mark before the line" "(got $(Shown-Value $markB13))"
    $threadsBefore = Server-Threads
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $reply = Send-Lines 'A' @('set -t B: @probe p13') 3000
    $watch.Stop()
    Start-Sleep -Milliseconds 1000
    $threadsAfter = Server-Threads
    $statusA13 = Status-Message 'A'
    $statusB13 = Status-Message 'B'
    Check ($reply -ceq "OK`n") "r13 shared A replies OK" "(reply [$(Shown $reply)])"
    Check ($watch.ElapsedMilliseconds -lt 2000) "r13 shared reply comes in 2s" "(took $($watch.ElapsedMilliseconds) ms)"
    Check ($threadsAfter -eq $threadsBefore) "r13 shared server thread count stays the same" "(threads $threadsBefore then $threadsAfter)"
    Check ($statusA13 -ceq 'mark13A') "r13 shared A status message is still the mark" "(got $(Shown-Value $statusA13))"
    Check ($statusB13 -ceq 'mark13B') "r13 shared B status message is still the mark" "(got $(Shown-Value $statusB13))"
    Check-Values 'r13' @{ 'A' = $null; 'B' = 'p13' }
    $cli13 = P @('set-option', '-t', 'A', '@own13', 'c13')
    $statusCli13 = Status-Message 'A'
    Check ($cli13.Code -eq 0) "r13 shared CLI set-option -t A exits 0" "(exit $($cli13.Code) [$($cli13.Text)])"
    Check ($statusCli13 -ceq 'mark13A') "r13 shared A status message is still the mark after CLI set-option -t A" "(got $(Shown-Value $statusCli13))"
    Check-Values 'r13' @{ 'A.@own13' = 'c13' }
} catch {
    $script:Failures++
    Write-Output "FAIL: script error $($_.Exception.Message)"
} finally {
    if ($script:Isolated) {
        Stop-Server
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
            $script:Failures++
            Write-Output "FAIL: could not remove $Root ($($_.Exception.Message))"
        }
    }
}

if ($script:Failures -gt 0) { Write-Output "$($script:Failures) check(s) failed"; exit 1 }
exit 0
