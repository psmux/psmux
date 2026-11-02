# PR #966 follow ups: what the server owned CLI run-shell must not change.
#
# 1. A bash job's `pwd` is a POSIX path. Setting PWD to the Windows job
#    directory made Git Bash print `C:\Users/...` (bash takes PWD as its
#    logical directory when it names the same place).
# 2. No server: other commands keep the exact "no server running on session
#    'x'" message, and run-shell runs the command in the CLI as before.
# 3. -b never opens the modal popup: a successful -b job shows nothing, a
#    failing one shows "'cmd' returned N" on the status line, keys typed after
#    it reach the pane, and a chatty -b hook does not pop. A foreground
#    run-shell still shows its output and failure in the popup.
# 4. With -OldServerExe: a new CLI against a server from before the native
#    reply keeps working the old way (plain command in the CLI with its exit
#    code, a format command and -b through the server) and runs each once.
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
$conf = Join-Path $work 'test.conf'
Set-Content $conf 'set -g display-time 60000'

$script:pass = 0; $script:fail = 0
function Pass($m) { $script:pass++; Write-Host "  PASS: $m" -ForegroundColor Green }
function Fail($m) { $script:fail++; Write-Host "  FAIL: $m" -ForegroundColor Red }
function Check($ok, $m) { if ($ok) { Pass $m } else { Fail $m } }
function WaitFor([scriptblock]$cond, [int]$ms = 8000) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($ms)
    do { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 } while ([DateTime]::UtcNow -lt $deadline)
    return $false
}
function State([string]$name, [string]$session) {
    $port = [int](Get-Content (Join-Path $env:PSMUX_DATA_DIR "${name}__${session}.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "${name}__${session}.key") -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', $port); $tcp.ReceiveTimeout = 5000
    try {
        $w = [IO.StreamWriter]::new($tcp.GetStream()); $w.AutoFlush = $true
        $r = [IO.StreamReader]::new($tcp.GetStream())
        $w.WriteLine("AUTH $key"); $null = $r.ReadLine(); $w.WriteLine('PERSISTENT'); $w.WriteLine('dump-state')
        for ($i = 0; $i -lt 80; $i++) {
            $l = $r.ReadLine()
            if ($null -eq $l) { break }
            if ($l.StartsWith('{') -and $l.Contains('popup_lines')) { return ($l | ConvertFrom-Json) }
        }
    } catch {} finally { $tcp.Dispose() }
    return $null
}
function PaneHas([string]$text) { ((& $Exe -L $ns capture-pane -p -t t) -match [regex]::Escape($text)).Count -gt 0 }

try {
    & $Exe -L $ns -f $conf new-session -d -s t -x 150 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1500

    Write-Host "`n[1] bash job pwd is a POSIX path" -ForegroundColor Yellow
    $bash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
    if (Test-Path $bash) {
        $here = New-Item -ItemType Directory -Force (Join-Path $work 'job dir')
        Push-Location $here
        $out = (& $Exe -L $ns run-shell "& '$bash' -c 'pwd'" 2>&1 | Out-String).Trim()
        Pop-Location
        Check ($out -match '^/') "pwd printed '$out'"
        Check ($out -notmatch '\\') 'no backslash in the bash path'
    } else {
        Write-Host "  SKIP: no Git Bash at $bash"
    }

    Write-Host "`n[3] -b: no popup, only a failure on the status line" -ForegroundColor Yellow
    # Keys first, with no dump-state connection in between: this is exactly
    # the sequence that lost the keys while the popup was up.
    # With the popup up the typed text still showed but Enter went to the
    # popup, so the command never ran (0 of 5 on the unfixed PR).
    $ran = 0
    foreach ($i in 1..3) {
        & $Exe -L $ns run-shell -b "cmd /c echo CHATTY_BG$i" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        & $Exe -L $ns send-keys -t t "echo AFTERPOP_$i" Enter
        if (WaitFor { ((& $Exe -L $ns capture-pane -p -t t -S -100) -match "^AFTERPOP_$i").Count -gt 0 } 3000) { $ran++ }
    }
    Check ($ran -eq 3) "commands typed after a chatty -b job run ($ran of 3)"
    $st = State $ns 't'
    Check ($st -and -not $st.popup_active -and -not (($st.popup_lines -join ' ').Contains('CHATTY_BG'))) 'successful -b output opens no popup'
    & $Exe -L $ns set-hook -g after-new-window 'run-shell -b "cmd /c echo HOOKOUT"' 2>&1 | Out-Null
    & $Exe -L $ns new-window -d 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1500
    $st = State $ns 't'
    Check ($st -and -not $st.popup_active -and -not (($st.popup_lines -join ' ').Contains('HOOKOUT'))) 'chatty after-new-window -b hook opens no popup'
    & $Exe -L $ns set-hook -gu after-new-window 2>&1 | Out-Null
    & $Exe -L $ns run-shell -b 'cmd /c echo FAILOUT & exit 3' 2>&1 | Out-Null
    Check (WaitFor { $s = State $ns 't'; $s -and "$($s.status_message)" -eq "'cmd /c echo FAILOUT & exit 3' returned 3" }) 'failing -b job: "returned 3" on the status line'
    $st = State $ns 't'
    Check ($st -and -not $st.popup_active) 'failing -b job opens no popup'
    & $Exe -L $ns send-keys -t t 'echo AFTERFAIL_OK' Enter
    Check (WaitFor { ((& $Exe -L $ns capture-pane -p -t t) -match '^AFTERFAIL_OK').Count -gt 0 }) 'keys typed after a failing -b job reach the pane'

    Write-Host "`n[3b] foreground binding route keeps the popup" -ForegroundColor Yellow
    $port = [int](Get-Content (Join-Path $env:PSMUX_DATA_DIR "${ns}__t.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "${ns}__t.key") -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    try {
        $w = [IO.StreamWriter]::new($tcp.GetStream()); $w.AutoFlush = $true
        $r = [IO.StreamReader]::new($tcp.GetStream())
        $w.WriteLine("AUTH $key"); $null = $r.ReadLine(); $w.WriteLine('PERSISTENT')
        $w.WriteLine("run-shell 'cmd /c exit 4'")
        Check (WaitFor { $s = State $ns 't'; $s -and (($s.popup_lines -join ' ').Contains("'cmd /c exit 4' returned 4")) }) 'foreground failure from a binding shows in the popup'
    } finally { $tcp.Dispose() }
    & $Exe -L $ns kill-server 2>&1 | Out-Null

    Write-Host "`n[2] no server" -ForegroundColor Yellow
    $none = "${ns}_none"
    $out = (& $Exe -L $none display-message -p x 2>&1 | Out-String).Trim()
    Check ($LASTEXITCODE -ne 0 -and $out -match "no server running on session '${none}__default'$") "display-message: '$out'"
    $out = (& $Exe -L $none run-shell 'cmd /c echo NOSERVER_RAN & exit 3' 2>&1 | Out-String).Trim()
    $code = $LASTEXITCODE
    Check ($out -eq 'NOSERVER_RAN' -and $code -eq 3) "run-shell runs in the CLI with its exit code (exit $code, '$out')"

    if ($OldServerExe) {
        Write-Host "`n[4] new CLI against an older server" -ForegroundColor Yellow
        $old = "${ns}_old"
        & $OldServerExe -L $old -f $conf new-session -d -s o | Out-Null
        Start-Sleep -Milliseconds 1500
        try {
            $count = Join-Path $work 'old count.txt'
            # pwsh is the run-shell shell here: plain pwsh statements, no redirection.
            $out = (& $Exe -L $old run-shell "Add-Content -LiteralPath '$count' X; Write-Output OLD_PLAIN; exit 5" 2>&1 | Out-String).Trim()
            $code = $LASTEXITCODE
            Check ($out -eq 'OLD_PLAIN' -and $code -eq 5) "plain command works with its exit code (exit $code, '$out')"
            Check (@(Get-Content $count -EA SilentlyContinue).Count -eq 1) 'plain command ran exactly once'
            $out = (& $Exe -L $old run-shell "cmd /c echo FMT_#{session_name}" 2>&1 | Out-String).Trim()
            Check ($out -eq 'FMT_o') "format command is expanded by the older server ('$out')"
            $bg = Join-Path $work 'old bg.txt'
            $sw = [Diagnostics.Stopwatch]::StartNew()
            & $Exe -L $old run-shell -b "Start-Sleep -Milliseconds 1200; Add-Content -LiteralPath '$bg' X" 2>&1 | Out-Null
            $ms = $sw.ElapsedMilliseconds
            Check ($LASTEXITCODE -eq 0 -and $ms -lt 900) "-b returns at once ($ms ms)"
            Check (WaitFor { Test-Path $bg }) '-b job ran on the older server'
            Start-Sleep -Milliseconds 500
            Check (@(Get-Content $bg -EA SilentlyContinue).Count -eq 1) '-b job ran exactly once'
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
