# PR #966 follow ups: what the server owned CLI run-shell must not change.
#
# 1. A bash job's `pwd` is a POSIX path. Setting PWD to the Windows job
#    directory made Git Bash print `C:\Users/...` (bash takes PWD as its
#    logical directory when it names the same place).
# 2. "no server running on session 'x'" stays the exact message every CLI
#    command printed, with no OS error text after it.
# 3. With -OldServerExe: a new CLI against a server from before the run-shell
#    handshake says to restart the server, not only "Invalid session key".
#
# Isolated: a unique -L namespace and a private PSMUX_DATA_DIR. Cleanup only
# through `-L <ns> kill-server`; nothing is killed by name.
param([string]$Exe, [string]$OldServerExe)

$ErrorActionPreference = 'Continue'
if (-not $Exe) { $Exe = $env:PSMUX_TEST_BIN }
if (-not $Exe) { $Exe = (Get-Command psmux -EA Stop).Source }
$ns = 'pr966f_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$work = Join-Path $env:TEMP $ns
New-Item -ItemType Directory -Force $work | Out-Null
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = Join-Path $work 'data'
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = '1'
foreach ($n in 'TMUX','TMUX_PANE','PSMUX_SESSION','PSMUX_TARGET_SESSION','PSMUX_TARGET_FULL','PSMUX_PANE') { Remove-Item "Env:$n" -EA SilentlyContinue }
$conf = Join-Path $work 'empty.conf'; Set-Content $conf ''

$script:pass = 0; $script:fail = 0
function Pass($m) { $script:pass++; Write-Host "  PASS: $m" -ForegroundColor Green }
function Fail($m) { $script:fail++; Write-Host "  FAIL: $m" -ForegroundColor Red }

try {
    Write-Host "`n[1] bash job pwd is a POSIX path" -ForegroundColor Yellow
    $bash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
    & $Exe -L $ns -f $conf new-session -d -s t | Out-Null
    Start-Sleep -Milliseconds 1500
    if (Test-Path $bash) {
        $here = New-Item -ItemType Directory -Force (Join-Path $work 'job dir')
        Push-Location $here
        $out = (& $Exe -L $ns run-shell "& '$bash' -c 'pwd'" 2>&1 | Out-String).Trim()
        Pop-Location
        if ($out -match '^/') { Pass "pwd printed '$out'" } else { Fail "pwd printed '$out'" }
        if ($out -notmatch '\\') { Pass 'no backslash in the bash path' } else { Fail "backslash in '$out'" }
    } else {
        Write-Host "  SKIP: no Git Bash at $bash"
    }
    & $Exe -L $ns kill-server 2>&1 | Out-Null

    Write-Host "`n[2] no server: the message every command printed" -ForegroundColor Yellow
    foreach ($cmd in @(@('display-message','-p','x'), @('run-shell','cmd /c echo hi'))) {
        $out = (& $Exe -L "${ns}_none" @cmd 2>&1 | Out-String).Trim()
        $code = $LASTEXITCODE
        if ($code -ne 0) { Pass "$($cmd[0]) fails ($code)" } else { Fail "$($cmd[0]) exit 0: $out" }
        if ($out -match "no server running on session '${ns}_none__default'$") { Pass "$($cmd[0]): '$out'" } else { Fail "$($cmd[0]): '$out'" }
    }

    if ($OldServerExe) {
        Write-Host "`n[3] new CLI against an older server" -ForegroundColor Yellow
        $old = "${ns}_old"
        & $OldServerExe -L $old -f $conf new-session -d -s o | Out-Null
        Start-Sleep -Milliseconds 1500
        try {
            $marker = Join-Path $work 'old ran.txt'
            $out = (& $Exe -L $old run-shell "cmd /c echo x > `"$marker`"" 2>&1 | Out-String).Trim()
            $code = $LASTEXITCODE
            if ($code -ne 0) { Pass "fails ($code)" } else { Fail 'exit 0' }
            if ($out -match 'older psmux' -and $out -match 'kill-server') { Pass "says to restart: '$out'" } else { Fail "message: '$out'" }
            Start-Sleep -Milliseconds 500
            if (-not (Test-Path $marker)) { Pass 'command not run anywhere' } else { Fail 'command ran' }
        } finally {
            & $OldServerExe -L $old kill-server 2>&1 | Out-Null
        }
    }
}
finally {
    & $Exe -L $ns kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir
}
Write-Host "`nResult: $script:pass passed, $script:fail failed"
if ($script:fail -gt 0) { exit 1 }
