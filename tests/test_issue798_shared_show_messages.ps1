param(
    [string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }),
    [string]$Result = $(Join-Path $env:TEMP "psmux_shared_show_messages_rows.txt")
)

if ($PSVersionTable.PSVersion.Major -lt 7) { Write-Host "needs PowerShell 7" -ForegroundColor Red; exit 2 }
$ErrorActionPreference = "Continue"
$script:Rows = [System.Collections.Generic.List[string]]::new()
$script:Version = $null

function Row($id, $state, $kind, $reason) {
    $line = "ROW $id $state $kind $reason"
    $script:Rows.Add($line)
    $color = switch ($state) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
    Write-Host "  $line" -ForegroundColor $color
}

$envNames = 'PSMUX_DATA_DIR', 'PSMUX_SERVER_MODE', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_DEBUG', 'PSMUX_PSREADLINE_HISTORY',
    'USERPROFILE', 'HOME', 'TEMP', 'TMP', 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME'
$saved = @{}
foreach ($v in $envNames) { $saved[$v] = [Environment]::GetEnvironmentVariable($v, 'Process') }
function RestoreEnv {
    foreach ($v in $envNames) {
        if ($null -eq $saved[$v]) { Remove-Item "Env:\$v" -EA SilentlyContinue } else { Set-Item "Env:\$v" $saved[$v] }
    }
}
function ClearSwitchEnv {
    foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME', 'PSMUX_DATA_DIR', 'PSMUX_SERVER_MODE', 'PSMUX_SERVER_DEBUG') {
        Remove-Item "Env:\$v" -EA SilentlyContinue
    }
}
ClearSwitchEnv
function DefaultLs {
    $out = @(& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | ForEach-Object { "$_".Trim() } | Where-Object { $_ })
    $code = $LASTEXITCODE
    if ($code -eq 0 -and @($out | Where-Object { $_ -notmatch ' \d+$' }).Count -eq 0) { return , $out }
    if ($code -ne 0 -and $out.Count -eq 1 -and $out[0] -match '^(psmux: )?no server running') { return , @() }
    $null
}
$defaultBefore = DefaultLs

$root = Join-Path $env:TEMP "psmux_showmsgs_$(Get-Random)"
$profileDir = Join-Path $root 'home'
$data = Join-Path $root 'data'
$tmpDir = Join-Path $root 'tmp'
$outDir = Join-Path $root 'out'
New-Item -ItemType Directory -Force $profileDir, $data, $tmpDir, $outDir | Out-Null
$empty = Join-Path $root 'empty.conf'
'' | Set-Content $empty
$env:USERPROFILE = $profileDir
$env:HOME = $profileDir
$env:TEMP = $tmpDir
$env:TMP = $tmpDir
$env:PSMUX_PSREADLINE_HISTORY = Join-Path $root 'history.txt'
$env:PSMUX_CONFIG_FILE = $empty
$env:PSMUX_SERVER_MODE = 'shared'
$env:PSMUX_DATA_DIR = $data
$script:Ns = "showmsgs$(Get-Random)"
$script:ServerPid = $null

function P { & $Exe -L $script:Ns @args 2>&1 }
function Has($s) { P has-session -t "=$s" | Out-Null; $LASTEXITCODE -eq 0 }
function Fmt($t, $f) { ((P display -p -t $t $f) -join '').Trim() }
function WaitFor([scriptblock]$cond, [int]$ms = 3000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 50 }
    return [bool](& $cond)
}
function RegistryFile($s, $ext) { Join-Path $data "$($script:Ns)__$s.$ext" }
function ServerPidOf($s) {
    $file = RegistryFile $s 'pid'
    if (-not (Test-Path -LiteralPath $file)) { return $null }
    $text = ((Get-Content -LiteralPath $file -Raw) ?? '').Trim()
    if ($text -match '^(\d+):') { return $Matches[1] }
    $null
}

function SummaryProblem([string[]]$lines, $s, $w) {
    if ($lines.Count -ne 6) { return "lines=$($lines.Count)" }
    if ($lines[0] -notmatch '^psmux \S+ \(Windows\)$') { return "line1='$($lines[0])'" }
    if ($null -eq $script:Version) { $script:Version = $lines[0] }
    elseif ($lines[0] -cne $script:Version) { return "version='$($lines[0])' first='$($script:Version)'" }
    if ($lines[1] -cne "pid: $($script:ServerPid)") { return "line2='$($lines[1])'" }
    if ($lines[2] -cne "session: $s") { return "line3='$($lines[2])'" }
    if ($lines[3] -cne "windows: $w") { return "line4='$($lines[3])'" }
    if ($lines[4] -notmatch '^uptime: \d+s$') { return "line5='$($lines[4])'" }
    $socket = "socket: $(RegistryFile $s 'port')"
    if ($lines[5] -cne $socket) { return "line6='$($lines[5])' want='$socket'" }
    ''
}

function CliRow($id, $s, $w, [string[]]$cmd) {
    $o = Join-Path $outDir "$id.out"
    $e = Join-Path $outDir "$id.err"
    & $Exe -L $script:Ns @cmd 1> $o 2> $e
    $code = $LASTEXITCODE
    $lines = @(Get-Content -LiteralPath $o -EA SilentlyContinue -ErrorVariable readErr)
    $err = (Get-Content -LiteralPath $e -Raw -EA SilentlyContinue -ErrorVariable +readErr) ?? ''
    if ($readErr.Count -ne 0) { Row $id INVALID read "exit=$code $($readErr[0].Exception.Message)"; return }
    if ($code -eq 0 -and $err -eq '') {
        $why = SummaryProblem $lines $s $w
        if ($why -eq '') { Row $id PASS ok "exit=0"; return }
    }
    else { $why = "exit=$code stderr='$($err.Trim())'" }
    if ($lines.Count -eq 1 -and $lines[0] -ceq 'ERROR: unknown command: show-messages') { Row $id FAIL unsupported "exit=$code $($lines[0])" }
    elseif ($lines.Count -eq 0) { Row $id FAIL empty $why }
    else { Row $id FAIL wrong $why }
}

function Raw($s, $line) {
    try {
        $port = [int]((Get-Content -LiteralPath (RegistryFile $s 'port') -Raw).Trim())
        $key = (Get-Content -LiteralPath (RegistryFile $s 'key') -Raw).Trim()
    }
    catch { return @{ State = 'INVALID'; Why = "registry of ${s}: $($_.Exception.Message)" } }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect('127.0.0.1', $port)
        $stream = $client.GetStream()
        $stream.ReadTimeout = 3000
        $request = [System.Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
        $stream.Write($request, 0, $request.Length)
        $stream.Flush()
        $got = [System.IO.MemoryStream]::new()
        $buf = [byte[]]::new(4096)
        while (($n = $stream.Read($buf, 0, $buf.Length)) -gt 0) { $got.Write($buf, 0, $n) }
        @{ State = 'ok'; Text = [System.Text.Encoding]::UTF8.GetString($got.ToArray()) }
    }
    catch {
        $cause = $_.Exception
        $timedOut = $false
        while ($cause) {
            if ($cause -is [System.Net.Sockets.SocketException] -and $cause.SocketErrorCode -eq [System.Net.Sockets.SocketError]::TimedOut) { $timedOut = $true }
            $cause = $cause.InnerException
        }
        if ($timedOut) { @{ State = 'TIMEOUT'; Why = 'no EOF within 3000 ms' } }
        else { @{ State = 'INVALID'; Why = $_.Exception.Message } }
    }
    finally { $client.Dispose() }
}

function Shown($text) { ($text -replace "`r", '\r' -replace "`n", '\n') }

function RawRow($id, $line, $s, $w) {
    $name = ($line -split ' ')[0]
    $r = Raw 'A' $line
    if ($r.State -ne 'ok') { Row $id $r.State raw $r.Why; return }
    $text = $r.Text
    if ($text -ceq "OK`nERROR: unknown command: $name`n") { Row $id FAIL unsupported (Shown $text); return }
    if ($text -ceq "OK`n") { Row $id FAIL empty (Shown $text); return }
    if ($text.Contains("`r") -or -not $text.StartsWith("OK`n") -or -not $text.EndsWith("`n")) { Row $id FAIL wrong (Shown $text); return }
    $body = $text.Substring(3, $text.Length - 4)
    $why = SummaryProblem ([string[]]($body -split "`n")) $s $w
    if ($why -eq '') { Row $id PASS ok 'bytes' } else { Row $id FAIL wrong "$why text=$(Shown $text)" }
}

function CheckRow($id, $line) {
    $r = Raw 'A' $line
    if ($r.State -ne 'ok') { Row $id $r.State raw $r.Why; return }
    $text = $r.Text
    if ($text.StartsWith("OK`n") -and $text.Substring(3).Trim() -ceq 'B') { Row $id PASS ok 'B' }
    else { Row $id INVALID target (Shown $text) }
}

$rowIds = 'R1', 'R2', 'R3a', 'R3b', 'R3c', 'R4', 'R5a', 'R5b', 'R5c', 'R5d', 'C1', 'C2'

try {
    $why = ''
    P new-session -d -s A -x 120 -y 30 | Out-Null
    if (-not (WaitFor { Has A } 8000)) { $why = 'A did not start' }
    if (-not $why) {
        P new-session -d -s B -x 120 -y 30 | Out-Null
        if (-not (WaitFor { Has B } 8000)) { $why = 'B did not start' }
    }
    if (-not $why) {
        P new-window -d -t 'B:' | Out-Null
        if (-not (WaitFor { (Fmt 'B:' '#{session_windows}') -eq '2' } 3000)) { $why = "B windows=$(Fmt 'B:' '#{session_windows}')" }
    }
    if (-not $why) {
        $files = foreach ($s in 'A', 'B') { foreach ($ext in 'port', 'key', 'pid') { RegistryFile $s $ext } }
        if (-not (WaitFor { @($files | Where-Object { -not (Test-Path -LiteralPath $_) }).Count -eq 0 } 8000)) { $why = 'registry files missing' }
    }
    if (-not $why) {
        $pidA = ServerPidOf 'A'
        $pidB = ServerPidOf 'B'
        if ($null -eq $pidA -or $pidA -ne $pidB) { $why = "server pid A=$pidA B=$pidB" } else { $script:ServerPid = $pidA }
    }
    if (-not $why) {
        $sid = Fmt 'B:' '#{session_id}'
        if ($sid -notmatch '^\$\d+$') { $why = "B session_id='$sid'" }
    }
    if ($why) {
        foreach ($id in $rowIds) { Row $id INVALID setup $why }
    }
    else {
        CliRow R1 'A' 1 @('show-messages', '-t', 'A')
        CliRow R2 'B' 2 @('show-messages', '-t', 'B')
        CliRow R3a 'A' 1 @('showmsgs', '-t', 'A')
        CliRow R3b 'A' 1 @('server-info', '-t', 'A')
        CliRow R3c 'A' 1 @('info', '-t', 'A')
        RawRow R4 'show-messages' 'A' 1
        RawRow R5a 'show-messages -t B' 'A' 1
        RawRow R5b 'showmsgs -t B:' 'A' 1
        RawRow R5c ('show-messages -t ' + $sid + ':') 'A' 1
        RawRow R5d ('showmsgs -t ' + $sid) 'A' 1
        CheckRow C1 "display-message -p -t B: '#{session_name}'"
        CheckRow C2 ('display-message -p -t ' + $sid + ": '#{session_name}'")
    }
} finally {
    P kill-server | Out-Null
    $gone = $false
    if ($script:ServerPid) { $gone = WaitFor { -not (Get-Process -Id ([int]$script:ServerPid) -EA SilentlyContinue) } 5000 }
    RestoreEnv
    ClearSwitchEnv
    $defaultAfter = DefaultLs
    RestoreEnv
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

foreach ($id in $rowIds) {
    if (@($script:Rows | Where-Object { ($_ -split ' ')[1] -ceq $id }).Count -eq 0) { Row $id INVALID missing 'no row was written' }
}

if (-not $script:ServerPid) { Row LEFT INVALID same 'no server pid was read' }
elseif ($gone) { Row LEFT PASS same "server pid=$($script:ServerPid)" }
else { Row LEFT FAIL same "still running: $($script:ServerPid)" }
if ($null -eq $defaultBefore -or $null -eq $defaultAfter) { Row DEFAULT INVALID same "the default ls could not be read: before=$($null -ne $defaultBefore) after=$($null -ne $defaultAfter)" }
else {
    $lost = @($defaultBefore | Where-Object { $defaultAfter -cnotcontains $_ })
    $added = @($defaultAfter | Where-Object { $defaultBefore -cnotcontains $_ })
    $used = @($added | Where-Object { @('A', 'B') -contains ($_ -replace ' \d+$', '') })
    if ($lost.Count -eq 0 -and $used.Count -eq 0) { Row DEFAULT PASS same "ok, other new sessions='$($added -join ',')'" }
    else { Row DEFAULT FAIL same "lost='$($lost -join ',')' test names='$($used -join ',')'" }
}

try { $script:Rows | Set-Content -Path $Result -Encoding utf8 -ErrorAction Stop }
catch { Write-Host "cannot write ${Result}: $($_.Exception.Message)" -ForegroundColor Red; exit 2 }
$states = @($script:Rows | ForEach-Object { ($_ -split ' ')[2] })
Write-Host "`n=== Results: $Result ===" -ForegroundColor Cyan
if ($states -contains 'INVALID' -or $states -contains 'TIMEOUT') { exit 2 }
if ($states -contains 'FAIL') { exit 1 }
exit 0
