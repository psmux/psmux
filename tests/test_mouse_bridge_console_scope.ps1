param([string]$PsmuxExe = $env:PSMUX_EXE)
$ErrorActionPreference = 'Stop'
if (-not $PsmuxExe) { $PsmuxExe = (Get-Command psmux -ErrorAction Stop).Source }
$ns = 'mouse-console-' + [guid]::NewGuid().ToString('N').Substring(0,12)
$dir = Join-Path $env:TEMP $ns
$registry = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE '.psmux' }
function Default-Sessions {
    $saved = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        @(& $PsmuxExe list-sessions -F '#{session_name}' 2>$null) | Sort-Object
    } finally { $ErrorActionPreference = $saved }
}
$before = @(Default-Sessions)
$helpers = @()
$failed = 0
New-Item -ItemType Directory $dir | Out-Null

function Send-Mouse($session, $pane) {
    # Private test namespace only; never print the local server's auth key.
    $base = Join-Path $registry ($ns + '__' + $session)
    $port = [int](Get-Content ($base + '.port') -Raw).Trim()
    $key = (Get-Content ($base + '.key') -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    try {
        $tcp.ReceiveTimeout = 5000
        $w = [IO.StreamWriter]::new($tcp.GetStream())
        $r = [IO.StreamReader]::new($tcp.GetStream())
        $w.WriteLine("AUTH $key"); $w.Flush()
        if ($r.ReadLine() -ne 'OK') { throw 'Test server authentication failed' }
        1..3 | ForEach-Object { $w.WriteLine("pane-mouse $pane 35 10 5 M") }
        $w.Flush()
        Start-Sleep -Milliseconds 300
    } finally { $tcp.Dispose() }
}
try {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) 'csc.exe'
    if (-not (Test-Path $csc)) { $csc = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe' }
    $child = Join-Path $dir 'native.exe'
    & $csc /nologo /optimize "/out:$child" (Join-Path $PSScriptRoot 'mouse_bridge_console_child.cs')
    if ($LASTEXITCODE -ne 0) { throw 'Fixture compilation failed' }
    Copy-Item $child (Join-Path $dir 'ssh.exe')
    foreach ($case in @('native','private-bridge','bridge')) {
        $log = Join-Path $dir ($case + '.log')
        $exe = if ($case -eq 'bridge') { Join-Path $dir 'ssh.exe' } else { $child }
        $command = '"' + $exe + '" ' + $case + ' "' + $log + '"'
        & $PsmuxExe -L $ns -f NUL new-session -d -s $case -x 100 -y 30 $command
        if ($LASTEXITCODE -ne 0) { throw "Could not start $case in test namespace" }
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        do {
            Start-Sleep -Milliseconds 200
            $content = if (Test-Path $log) { Get-Content $log -Raw } else { '' }
        } until ($content -match 'READY' -or [DateTime]::UtcNow -gt $deadline)
        if ($content -notmatch 'READY') { throw "Fixture $case did not become ready" }
        if ($content -match 'HELPER (\d+)') {
            $helperPid = [int]$Matches[1]; $helpers += $helperPid
            if ($content -match "(?m)^MEMBER $helperPid$") { throw 'Helper unexpectedly shares pane console' }
        }
        & $PsmuxExe -L $ns set-option -t $case mouse on
        $pane = ((& $PsmuxExe -L $ns list-panes -t ($case + ':0') -F '#{pane_id}') | Select-Object -First 1).Trim().TrimStart('%')
        Send-Mouse $case $pane
        & $PsmuxExe -L $ns send-keys -t ($case + ':0') Q
        Start-Sleep -Milliseconds 400
        $content = Get-Content $log -Raw
        if ($case -eq 'bridge') {
            $ok = $content -match 'BYTES .*1B-5B-3C' -and $content -match 'BYTES .*51'
        } else {
            $ok = $content -notmatch 'KEY 001B' -and $content -match 'KEY 0051' -and $content -match '(?m)^MOUSE$'
        }
        if ($ok) { Write-Host "PASS $case : correct mouse transport and keyboard delivery" }
        else { $failed++; Write-Host "FAIL $case"; Write-Host $content }
    }
} finally {
    & $PsmuxExe -L $ns kill-server 2>$null | Out-Null
    foreach ($helperPid in $helpers) {
        $p = Get-Process -Id $helperPid -ErrorAction SilentlyContinue
        if ($p -and $p.Path -eq (Join-Path $dir 'ssh.exe')) { Stop-Process -Id $helperPid -Force }
    }
    $after = @(Default-Sessions)
    if (Compare-Object @($before) @($after)) { throw 'Default sessions changed during isolated test' }
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failed) { throw "$failed mouse transport cases failed" }
