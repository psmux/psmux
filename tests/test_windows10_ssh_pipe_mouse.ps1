# Windows 10 SSH mouse E2E without a remote PTY. This models `ssh -T`: psmux
# receives anonymous stdin/stdout pipes plus SSH_CONNECTION, so ConPTY is not
# present. Raw SGR bytes must enter copy mode and keep scrolling.
param(
    [string]$Psmux = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\psmux.exe')
)

$ErrorActionPreference = 'Stop'
# A per run namespace so a parallel or leftover run never collides.
$Namespace = "e2e_win10_ssh_pipe_$(Get-Random -Maximum 999999)"
$Session = 'pipe_mouse'
$script:Passed = 0
$script:Failed = 0

function Pass([string]$Message) {
    Write-Host "  [PASS] $Message" -ForegroundColor Green
    $script:Passed++
}

function Fail([string]$Message) {
    Write-Host "  [FAIL] $Message" -ForegroundColor Red
    $script:Failed++
}

function Send-Bytes($Process, [byte[]]$Bytes) {
    $stream = $Process.StandardInput.BaseStream
    $stream.Write($Bytes, 0, $Bytes.Length)
    $stream.Flush()
}

function Vt-Bytes([string]$Tail) {
    return [byte[]](@(0x1b) + [Text.Encoding]::ASCII.GetBytes($Tail))
}

function Wait-ForHistory([int]$Minimum = 50) {
    for ($i = 0; $i -lt 50; $i++) {
        $raw = & $Psmux -L $Namespace list-panes -t $Session -F '#{history_size}' 2>$null
        $size = 0
        if ($LASTEXITCODE -eq 0 -and
            [int]::TryParse(($raw -join '').Trim(), [ref]$size) -and
            $size -ge $Minimum) {
            return $size
        }
        Start-Sleep -Milliseconds 200
    }
    throw "pane history did not reach $Minimum lines before the timeout"
}

function Get-State {
    $base = "${Namespace}__${Session}"
    $dataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE '.psmux' }
    $port = [int](Get-Content (Join-Path $dataDir "$base.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $dataDir "$base.key") -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $tcp.ReceiveTimeout = 4000
    try {
        $stream = $tcp.GetStream()
        $writer = [IO.StreamWriter]::new($stream)
        $reader = [IO.StreamReader]::new($stream)
        $writer.Write("AUTH $key`n")
        $writer.Flush()
        $null = $reader.ReadLine()
        $writer.Write("dump-state`n")
        $writer.Flush()
        for ($i = 0; $i -lt 100; $i++) {
            $line = $reader.ReadLine()
            if ($line -and $line -ne 'NC' -and $line.Length -gt 100) {
                return ($line | ConvertFrom-Json)
            }
        }
        throw 'dump-state returned no JSON'
    }
    finally {
        $tcp.Dispose()
    }
}

if (-not (Test-Path $Psmux)) {
    throw "psmux release binary not found: $Psmux"
}

Write-Host "`n=== Windows 10 SSH direct-pipe mouse E2E ===" -ForegroundColor Cyan
& $Psmux -L $Namespace kill-server 2>$null | Out-Null
$hadNoWarm = Test-Path Env:PSMUX_NO_WARM
$previousNoWarm = $env:PSMUX_NO_WARM
try {
    # Keep this isolated E2E from leaving an intentional warm standby process
    # that would lock the release binary after the namespace is killed.
    $env:PSMUX_NO_WARM = '1'
    & $Psmux -L $Namespace new-session -d -s $Session -- cmd.exe
}
finally {
    if ($hadNoWarm) { $env:PSMUX_NO_WARM = $previousNoWarm }
    else { Remove-Item Env:PSMUX_NO_WARM -ErrorAction SilentlyContinue }
}
Start-Sleep -Seconds 2
& $Psmux -L $Namespace set-option -g mouse on -t $Session | Out-Null
& $Psmux -L $Namespace set-option -g scroll-enter-copy-mode on -t $Session | Out-Null
& $Psmux -L $Namespace send-keys -t $Session 'for /L %i in (1,1,300) do @echo PIPE_HISTORY_%i' Enter | Out-Null
$historySize = Wait-ForHistory
Pass "pane history is ready (size=$historySize)"

$psi = [Diagnostics.ProcessStartInfo]::new()
$psi.FileName = (Resolve-Path $Psmux).Path
$psi.Arguments = "-L $Namespace attach -t $Session"
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables['SSH_CONNECTION'] = 'client 50000 server 22'
$psi.EnvironmentVariables['PSMUX_SSH_DEBUG'] = '1'

$process = [Diagnostics.Process]::Start($psi)
$stdout = [IO.MemoryStream]::new()
$stderr = [IO.MemoryStream]::new()
$stdoutTask = $process.StandardOutput.BaseStream.CopyToAsync($stdout)
$stderrTask = $process.StandardError.BaseStream.CopyToAsync($stderr)

try {
    Start-Sleep -Milliseconds 500
    Send-Bytes $process (Vt-Bytes '[8;30;100t')
    Start-Sleep -Seconds 3

    $hex = (($stdout.ToArray() | ForEach-Object { $_.ToString('x2') }) -join '')
    if ($hex.Contains('1b5b3f3130303068') -and
        $hex.Contains('1b5b3f3130303268') -and
        $hex.Contains('1b5b3f3130303668')) {
        Pass 'mouse-enable DECSET 1000/1002/1006 reached the raw output pipe'
    } else {
        Fail 'raw output pipe is missing a mouse-enable DECSET sequence'
    }
    if ($hex.Contains('1b5b313874')) { Pass 'XTWINOPS size query reached the raw output pipe' }
    else { Fail 'XTWINOPS size query missing from raw output pipe' }

    $before = Get-State
    Send-Bytes $process (Vt-Bytes '[<64;10;5M')
    Start-Sleep -Seconds 2
    $first = Get-State
    if (-not $before.layout.copy_mode -and $first.layout.copy_mode -and $first.layout.scroll_offset -gt 0) {
        Pass "SGR WheelUp entered copy mode (offset=$($first.layout.scroll_offset))"
    } else {
        Fail "WheelUp state before=$($before.layout.copy_mode) after=$($first.layout.copy_mode) offset=$($first.layout.scroll_offset)"
    }

    Send-Bytes $process (Vt-Bytes '[<64;10;5M')
    Start-Sleep -Seconds 1
    $second = Get-State
    if ($second.layout.scroll_offset -gt $first.layout.scroll_offset) {
        Pass "repeated WheelUp scrolled further ($($first.layout.scroll_offset) -> $($second.layout.scroll_offset))"
    } else {
        Fail "repeated WheelUp did not advance offset ($($first.layout.scroll_offset) -> $($second.layout.scroll_offset))"
    }

    & $Psmux -L $Namespace set-option -g mouse off -t $Session | Out-Null
    Start-Sleep -Seconds 1
    Send-Bytes $process (Vt-Bytes '[<64;10;5M')
    Start-Sleep -Seconds 1
    $disabled = Get-State
    if ($disabled.layout.scroll_offset -eq $second.layout.scroll_offset) {
        Pass 'mouse off ignored WheelUp without changing scroll position'
    } else {
        Fail "mouse off changed scroll offset ($($second.layout.scroll_offset) -> $($disabled.layout.scroll_offset))"
    }
}
finally {
    if (-not $process.HasExited) {
        Send-Bytes $process ([byte[]](0x02, 0x64))
        Start-Sleep -Milliseconds 700
    }
    if (-not $process.HasExited) { $process.Kill() }
    $process.Dispose()
    & $Psmux -L $Namespace kill-server 2>$null | Out-Null
}

Write-Host "`n=== Results: Passed=$script:Passed Failed=$script:Failed ===" -ForegroundColor Cyan
exit $script:Failed
