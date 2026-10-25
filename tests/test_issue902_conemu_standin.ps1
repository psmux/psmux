# Issue #902 without ConEmu installed: a stand-in host that behaves like ConEmu
# for the one thing that matters here.
#
# ConEmu brackets a paste with ESC[200~ and ESC[201~ only after the app asked
# for bracketed paste (ESC[?2004h), and it writes the whole paste into the
# real console with WriteConsoleInput as plain key records. The stand-in hosts
# the client under a pseudoconsole with ConEmuPID set (conptyfeed610.exe, which
# mirrors everything the client writes), reads whether the client asked for
# ?2004h, and then writes the paste the way ConEmu would (conemupaste902.exe):
# with the markers if asked, without them if not.
#
# Before the fix the client asks, so the pane is left with `PS C:\> [201~`
# (Windows PowerShell without a profile, where ESC clears the line). After it,
# a ConEmu client on the console route does not ask and the pane holds the
# text. With PSMUX_VT_INPUT=1 the client still asks and its VT parser decodes
# the markers, on every build. Without ConEmuPID the client still asks.
#
# Runs in its own -L namespace with a private PSMUX_DATA_DIR. Set
# PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$tag = [guid]::NewGuid().ToString('N').Substring(0, 6)
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_902s_" + $tag)
New-Item -ItemType Directory -Force $TMP | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$feed = Join-Path $TMP "conptyfeed610.exe"
$paste = Join-Path $TMP "conemupaste902.exe"
& $csc /nologo /platform:x64 /out:$feed "$PSScriptRoot\conptyfeed610.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$paste "$PSScriptRoot\conemupaste902.cs" 2>&1 | Out-Null
if (-not (Test-Path $feed) -or -not (Test-Path $paste)) {
    Write-Fail "could not build the C# harnesses (csc at $csc)"
    exit 1
}

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
$conf = Join-Path $TMP "s.conf"
"set -g default-shell '$POWERSHELL -NoLogo -NoProfile'`nset -g status-left ''`n" | Set-Content $conf -Encoding ASCII
$text = "Hello, psmux!"

# Returns @{ Asked = <client wrote ESC[?2004h>; Line = <last pane line> }.
function Paste-Via-StandIn([bool]$conemu, [bool]$vtInput) {
    $NS = "s902$tag" + [guid]::NewGuid().ToString('N').Substring(0, 4)
    $DATA = Join-Path $TMP "data_$NS"
    New-Item -ItemType Directory -Force $DATA | Out-Null
    $ctrl = Join-Path $TMP "$NS.ctrl"; $out = Join-Path $TMP "$NS.out"
    "" | Set-Content $ctrl
    $saved = @{}
    $names = 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX','WT_SESSION',
             'TERM_PROGRAM','WEZTERM_PANE','TERMINAL_EMULATOR','ALACRITTY_LOG','SSH_CONNECTION','SSH_CLIENT',
             'SSH_TTY','PSMUX_VT_INPUT','ConEmuPID','PSMUX_DATA_DIR'
    foreach ($n in $names) { $saved[$n] = [Environment]::GetEnvironmentVariable($n); Remove-Item "Env:\$n" -EA SilentlyContinue }
    $env:PSMUX_DATA_DIR = $DATA
    if ($conemu) { $env:ConEmuPID = "15604" }
    if ($vtInput) { $env:PSMUX_VT_INPUT = "1" }
    $p = Start-Process $feed -PassThru -WindowStyle Hidden -WorkingDirectory "$env:SystemDrive\" `
        -ArgumentList "`"$ctrl`" `"$out`" `"$PSMUX`" -L $NS -f `"$conf`" new-session -s s -x 100 -y 25"
    foreach ($n in $names) { if ($null -ne $saved[$n]) { Set-Item "Env:\$n" $saved[$n] } else { Remove-Item "Env:\$n" -EA SilentlyContinue } }
    $env:PSMUX_DATA_DIR = $DATA
    for ($i = 0; $i -lt 50; $i++) {
        Start-Sleep -Milliseconds 300
        & $PSMUX -L $NS has-session -t s 2>$null
        if ($LASTEXITCODE -eq 0) { break }
    }
    # The prompt has to be drawn before the paste lands after it.
    Start-Sleep -Seconds 5
    $cpid = (Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" |
        Where-Object { $_.CommandLine -match "-L $NS " } | Select-Object -First 1).ProcessId
    $fs = [IO.File]::Open($out, 'Open', 'Read', 'ReadWrite')
    $sr = New-Object IO.StreamReader($fs); $written = $sr.ReadToEnd(); $sr.Close()
    $asked = $written.Contains([char]27 + '[?2004h')
    $line = $null
    if ($cpid) {
        & $paste $cpid $(if ($asked) { "bracketed" } else { "plain" }) $text | Out-Null
        Start-Sleep -Seconds 2
        $line = (& $PSMUX -L $NS capture-pane -t s -p | Where-Object { $_ -match '\S' } | Select-Object -Last 1)
    }
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
    if ($null -ne $saved['PSMUX_DATA_DIR']) { $env:PSMUX_DATA_DIR = $saved['PSMUX_DATA_DIR'] }
    Add-Content $ctrl "QUIT"
    Start-Sleep -Seconds 1
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue }
    return @{ Asked = $asked; Line = $(if ($line) { "$line".TrimEnd() } else { $null }) }
}

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

Write-Host "`n--- 1. ConEmu on the console route ---" -ForegroundColor Yellow
$r = Paste-Via-StandIn $true $false
Write-Host "  [INFO] asked for ?2004h: $($r.Asked), pane line [$($r.Line)]"
if ($r.Asked) { Write-Fail "the client asked ConEmu for bracketed paste on the console route" }
else { Write-Pass "the client did not ask ConEmu for bracketed paste" }
if ($r.Line -and $r.Line -notmatch '20[01]~' -and $r.Line.EndsWith("> $text")) { Write-Pass "the pane holds the pasted text alone" }
else { Write-Fail "the pane line is [$($r.Line)]" }

Write-Host "`n--- 2. ConEmu with PSMUX_VT_INPUT=1 ---" -ForegroundColor Yellow
$r = Paste-Via-StandIn $true $true
Write-Host "  [INFO] asked for ?2004h: $($r.Asked), pane line [$($r.Line)]"
if ($r.Asked) { Write-Pass "the VT route still asks for bracketed paste" } else { Write-Fail "the VT route did not ask" }
if ($r.Line -and $r.Line -notmatch '20[01]~' -and $r.Line.EndsWith("> $text")) { Write-Pass "the VT parser decoded the markers" }
else { Write-Fail "the pane line is [$($r.Line)]" }

Write-Host "`n--- 3. not ConEmu ---" -ForegroundColor Yellow
$r = Paste-Via-StandIn $false $false
Write-Host "  [INFO] asked for ?2004h: $($r.Asked)"
if ($r.Asked) { Write-Pass "other hosts are still asked for bracketed paste" } else { Write-Fail "a non ConEmu host was not asked" }

Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
