# Issue #928: the sixel option takes off, on and auto, and auto, the default,
# draws only when the terminal's DA1 reply lists sixel, as tmux does.
#
# tmux has no sixel option. It turns its sixel feature on from a DA1 reply
# listing 4 after a level of 61 to 65 (tty-keys.c:1528 in tmux 3.8) and draws
# no image without it (tty_cmd_sixelimage, tty.c:2191), so a terminal that
# does not answer gets no images. psmux drew them whenever no reply came.
#
# Part 1, always run: the option over the CLI and dump-state.
#   1. the default is auto, in show-options and in the frame's sixel_mode,
#   2. off, on and auto are set and shown, and yes still reads as on,
#   3. any other value is refused and changes nothing,
#   4. set with no value swaps on and off and leaves auto alone.
#
# Part 2, needs PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll and
# OpenConsole.exe, see tests/test_issue914_sixel_conpty_dir.ps1): a pane on
# that host writes a sixel, and a client attached under tests/conpty_da1_host.cs,
# which answers DA1 as told, writes the image to its terminal or not:
#   the default with no reply does not draw (it did before) and with yes
#   draws, auto + yes draws, auto + no and auto + no reply do not, on + no reply
#   draws, off + yes does not.
#
# Isolation: private PSMUX_DATA_DIR and empty config, unique -L namespace on
# every command, cleanup only with `-L <ns> kill-server`.

$ErrorActionPreference = 'Continue'

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$ns = "tsx_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_sixel_auto_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$saved = @{}
foreach ($k in 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_CONPTY_DIR', 'PSMUX_SESSION', 'PSMUX_HOST_COLORS', 'TMUX', 'TMUX_PANE') {
    $saved[$k] = [Environment]::GetEnvironmentVariable($k)
    Remove-Item "Env:\$k" -ErrorAction SilentlyContinue
}
$env:PSMUX_DATA_DIR = $data
$env:PSMUX_CONFIG_FILE = Join-Path $data 'empty.conf'
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value '' -Encoding UTF8

$script:TestsPassed = 0
$script:TestsFailed = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:TestsPassed++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:TestsFailed++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>&1 }
function Show { (P show-options -g -v sixel | Out-String).Trim() }

function WaitSession($s) {
    for ($i = 0; $i -lt 60; $i++) {
        & $exe -L $ns has-session -t $s 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# One dump-state frame over the session's port.
function Frame($s) {
    $port = (Get-Content (Join-Path $data "${ns}__$s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $data "${ns}__$s.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    $tcp.ReceiveTimeout = 4000
    $st = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($st); $r = [System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("dump-state`n"); $w.Flush()
    $json = $null
    for ($j = 0; $j -lt 300; $j++) {
        try { $line = $r.ReadLine() } catch { break }
        if ($null -eq $line) { break }
        if ($line -ne 'NC' -and $line.Length -gt 100) { $json = $line; break }
    }
    $tcp.Close()
    if ($json) { return $json | ConvertFrom-Json }
    return $null
}

function Cleanup {
    & $exe -L $ns kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 500
    foreach ($k in $saved.Keys) {
        if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -ErrorAction SilentlyContinue }
    }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

Write-Host "`n=== sixel off/on/auto ===" -ForegroundColor Cyan
Write-Host "  exe: $exe ($(& $exe -V 2>$null | Select-Object -Last 1))"

# ── Part 1: the option ──────────────────────────────────────────────────────
Write-Host "`n--- 1. the option ---" -ForegroundColor Cyan
P new-session -d -s a -x 120 -y 30 | Out-Null
if (-not (WaitSession 'a')) { Check 'session starts' $false ''; Cleanup; exit $script:TestsFailed }

$v = Show
Check 'the default is auto' ($v -eq 'auto') "got '$v'"
$all = (P show-options -g | Out-String)
Check 'show-options -g lists sixel auto' ($all -match '(?m)^sixel auto\r?$') ''
$f = Frame 'a'
Check "the frame carries sixel_mode auto" ($f -and $f.sixel_mode -eq 'auto') "got '$($f.sixel_mode)'"
Check "and sixel true for an older client" ($f -and $f.sixel -eq $true) "got '$($f.sixel)'"

foreach ($pair in @(@('off', 'off', $false), @('on', 'on', $true), @('auto', 'auto', $true), @('yes', 'on', $true))) {
    $set, $want, $flag = $pair
    P set-option -g sixel $set | Out-Null
    $v = Show
    $f = Frame 'a'
    Check "set $set shows $want" ($v -eq $want) "got '$v'"
    Check "set ${set}: frame sixel_mode $want, sixel $flag" ($f -and $f.sixel_mode -eq $want -and $f.sixel -eq $flag) "got '$($f.sixel_mode)' / '$($f.sixel)'"
}

$err = (P set-option -g sixel always | Out-String).Trim()
Check 'set always is refused' ($err -match 'unknown value: always') "got '$err'"
$v = Show
Check 'and the value stays on' ($v -eq 'on') "got '$v'"

P set-option -g sixel | Out-Null
$v = Show
Check 'set with no value turns on into off' ($v -eq 'off') "got '$v'"
P set-option -g sixel | Out-Null
$v = Show
Check 'and off into on' ($v -eq 'on') "got '$v'"
P set-option -g sixel auto | Out-Null
P set-option -g sixel | Out-Null
$v = Show
Check 'and leaves auto alone' ($v -eq 'auto') "got '$v'"

P kill-server | Out-Null
Start-Sleep -Milliseconds 500

# ── Part 2: what the client writes ──────────────────────────────────────────
Write-Host "`n--- 2. what an attached client writes ---" -ForegroundColor Cyan
$conptyDir = $saved['PSMUX_CONPTY_DIR']
if ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $conptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST }
$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$hostExe = Join-Path $data 'conpty_da1_host.exe'
if ($csc) { & $csc.FullName /nologo /optimize /out:$hostExe (Join-Path $PSScriptRoot 'conpty_da1_host.cs') 2>&1 | Out-Null }

if (-not $env:PSMUX_CONPTY_DIR_FOR_TEST -or -not (Test-Path (Join-Path $conptyDir 'conpty.dll')) -or -not (Test-Path (Join-Path $conptyDir 'OpenConsole.exe'))) {
    Write-Host "  SKIP: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe" -ForegroundColor Yellow
} elseif (-not (Test-Path $hostExe)) {
    Check 'build tests\conpty_da1_host.cs' $false '(csc.exe unavailable)'
} else {
    # A two colour 16 by 12 pixel sixel, as in the 914 suite.
    $body = 'Pq#1;2;100;100;0#2;2;0;100;0#1~~@@vv@@~~@@~~$#2??}}GG}}??}}??-#1@@@@@@@@@@@@@@'
    $env:PSMUX_CONPTY_DIR = $conptyDir
    P new-session -d -s b -x 120 -y 30 | Out-Null
    Remove-Item Env:\PSMUX_CONPTY_DIR
    if (-not (WaitSession 'b')) { Check 'session starts on OpenConsole' $false '' }
    else {
        Start-Sleep -Seconds 2
        P send-keys -t b "[Console]::Out.Write([char]27 + '$body' + [char]27 + '\'); 'EMITTED'" Enter | Out-Null
        $ok = $false
        for ($i = 0; $i -lt 40; $i++) {
            if ((P capture-pane -p -t b | Out-String) -match '(?m)^EMITTED') { $ok = $true; break }
            Start-Sleep -Milliseconds 250
        }
        Check 'the pane wrote the sixel' $ok ''

        # `default` is the option never set: before the fix it drew with no
        # reply at all, where tmux writes no image.
        foreach ($case in @(
                @('default', 'none', $false), @('default', 'yes', $true),
                @('auto', 'yes', $true), @('auto', 'no', $false), @('auto', 'none', $false),
                @('on', 'none', $true), @('off', 'yes', $false))) {
            $mode, $da1, $want = $case
            if ($mode -eq 'default') { P set-option -gu sixel | Out-Null }
            else { P set-option -g sixel $mode | Out-Null }
            $out = Join-Path $data "client_${mode}_$da1.bin"
            # OpenConsole waits for a DA1 answer before it starts the client, so
            # no reply needs the longer hold.
            $hold = if ($da1 -eq 'none') { 7000 } else { 4000 }
            & $hostExe (Join-Path $conptyDir 'conpty.dll') $out $da1 8 $hold "`"$exe`" -L $ns attach -t b" | Out-Null
            $bytes = if (Test-Path $out) { [IO.File]::ReadAllBytes($out) } else { [byte[]]@() }
            $text = [Text.Encoding]::ASCII.GetString($bytes)
            # The image inside a DCS; the command line on screen holds the body too.
            $drew = $text -match "\x1bP[^\x1b]{0,60}#1;2;100;100;0"
            $what = if ($want) { 'draws' } else { 'does not draw' }
            Check "sixel $mode, DA1 $da1 : the client $what" ($drew -eq $want) "(wrote $($bytes.Length) bytes, image: $drew)"
        }
    }
}

Cleanup
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
