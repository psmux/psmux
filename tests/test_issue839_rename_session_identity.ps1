# Issue #839: after rename-session, the attached client still thought it was in
# the session it attached to.
#
# The client keeps its registry base (the .port file stem) from the attach. A
# rename moves the session's .port and .key files to the new name, so:
#   - prefix = and the prompt's choose-buffer looked the old base up, found no
#     .port file and showed nothing, although the session had buffers
#   - prefix ( and ) looked for the old base among the live sessions, found
#     none and did nothing
#   - choose-session drew no `*` on the client's own row, and choose-tree drew
#     no `(attached)` on its session and refused `x` on its windows
# Switching to another session and back cured all of it, because a fresh
# attach reads the new base.
#
# tmux 3.7c, measured through a nested client after `rename-session -t sesA
# sesX`: the client's session is sesX, prefix = lists the buffer, prefix s
# shows `sesX: 1 windows (attached)` first, and prefix ) moves to sesB. In tmux
# the client holds the session by pointer (server_client_set_session,
# server-client.c) and cmd-rename-session.c only replaces its name.
#
# GROUND TRUTH: the attached client's console (tests\conread.cs); keys go in
# through WriteConsoleInput (tests\injector.cs). The client's session comes
# from list-clients. The scenario runs twice: in the default namespace and
# under -L, each in a PSMUX_DATA_DIR of its own, with session names nobody
# else uses, so no other psmux session is touched.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_SWITCH_TO', 'PSMUX_CLIENT_LAST_SESSION', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rsi_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP

Write-Host "`n=== Issue #839: the client follows rename-session ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $TMP "injector.exe"
$READ = Join-Path $TMP "conread.exe"
& $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs")  2>&1 | Out-Null
if (-not ((Test-Path $INJ) -and (Test-Path $READ))) {
    Write-Fail "could not build tests\injector.cs and tests\conread.cs"
    exit 1
}

$BUFTEXT = "rename buffer alpha"

function Run-Scenario($label, $ns) {
    Write-Host "`n===== $label =====" -ForegroundColor Cyan
    $tag = [guid]::NewGuid().ToString('N').Substring(0, 4)
    $A = "ra$tag"; $B = "rb$tag"; $X = "rx$tag"; $XZ = "${X}Z"
    $nsArgs = if ($ns) { @('-L', $ns) } else { @() }
    $base = { param($n) if ($ns) { "${ns}__$n" } else { $n } }

    function P { & $PSMUX @nsArgs @args 2>&1 }
    function Screen { return [string[]](& $READ $script:cpid) }
    function Send($keys) { & $INJ $script:cpid $keys | Out-Null; Start-Sleep -Milliseconds 900 }
    function Has($screen, $text) { return [bool]($screen | Where-Object { $_.Contains($text) }) }
    function Client-Session {
        foreach ($s in (P list-sessions -F '#{session_name}')) {
            $c = ((P list-clients -t "$s" -F '#{client_session}') -join '').Trim()
            if ($c) { return $c }
        }
        return ""
    }
    # The rename has reached the client once its status line carries the new
    # name, which comes from the same frame the client now takes its base from.
    function Wait-Status($name) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 6000) {
            if (Has (Screen) "[$name]") { return $true }
            Start-Sleep -Milliseconds 150
        }
        return $false
    }
    function Check-Buffers($how, $keys) {
        Send $keys
        $sc = Screen
        Check ((Has $sc " choose-buffer (") -and (Has $sc $BUFTEXT)) "$how lists the buffer" `
            "$how shows no buffer list"
        Send "q"
    }
    function Check-SessionStar($name) {
        Send "^b{SLEEP:500}s"
        $sc = Screen
        $row = $sc | Where-Object { $_ -match "\d+\.\s+\*\s+.*$([regex]::Escape($name)):" } | Select-Object -First 1
        Check ([bool]$row) "prefix s marks $name with *" "no row marks $name with *"
        Send "q"
    }

    # prefix ) to the other session, then prefix ( back. The way back is only
    # checked once the way there worked: a ) that did nothing leaves the client
    # where ( would have taken it.
    function Cycle-Check($own) {
        Send "^b{SLEEP:500})"
        Start-Sleep -Milliseconds 2500
        $moved = (Client-Session) -eq $B
        Check $moved "prefix ) moved the client from $own to $B" "prefix ) left the client in [$(Client-Session)]"
        if ($moved) {
            Send "^b{SLEEP:500}("
            Start-Sleep -Milliseconds 2500
            Check ((Client-Session) -eq $own) "prefix ( moved it back to $own" "the client is in [$(Client-Session)]"
        }
    }

    P new-session -d -s $A | Out-Null
    Start-Sleep -Milliseconds 800
    P new-session -d -s $B | Out-Null
    Start-Sleep -Milliseconds 800
    P set-buffer -t $A $BUFTEXT | Out-Null

    $p = Start-Process -FilePath $PSMUX -ArgumentList (@($nsArgs) + @('attach', '-t', $A)) -PassThru
    $script:cpid = $p.Id
    Start-Sleep -Seconds 4

    try {
        Write-Head "control: before any rename"
        Check ((Client-Session) -eq $A) "the client is in $A" "the client is in [$(Client-Session)]"
        Check-Buffers "prefix =" "^b{SLEEP:500}="

        Write-Head "a CLI rename-session $A to $X"
        P rename-session -t $A $X | Out-Null
        Check (Wait-Status $X) "the status line reads [$X]" "the status line never read [$X]"
        Check ((Client-Session) -eq $X) "the client is in $X" "the client is in [$(Client-Session)]"
        Check-Buffers "prefix =" "^b{SLEEP:500}="
        Check-Buffers "the prompt's choose-buffer" "^b{SLEEP:500}:{SLEEP:500}choose-buffer{ENTER}"
        Check-SessionStar $X

        Send "^b{SLEEP:500}w"
        $sc = Screen
        Check ([bool]($sc | Where-Object { $_ -match "$([regex]::Escape($X)): \d+ windows \(attached\)" })) `
            "prefix w shows $X as (attached)" "no tree header reads $X ... (attached)"
        Send "x"
        $sc = Screen
        $asked = Has $sc "kill-window"
        Check $asked "x on the own window asks kill-window" "x asked nothing"
        Send "n"
        Send "q"
        if ($asked) {
            $nw = @(P list-windows -t $X).Count
            Check ($nw -eq 1) "answering n kept the window" "$X has $nw windows"
        }

        Write-Head "prefix ) and ( cycle from the renamed session"
        Cycle-Check $X

        Write-Head "a rename from the client's own overlay, $X to $XZ"
        Send "^b{SLEEP:500}`$"
        Send "Z{ENTER}"
        Check (Wait-Status $XZ) "the status line reads [$XZ]" "the status line never read [$XZ]"
        Check-Buffers "prefix =" "^b{SLEEP:500}="
        Check-SessionStar $XZ
        Cycle-Check $XZ

        Write-Head "a refused rename leaves the client where it is"
        $out = (P rename-session -t $XZ $B) -join ' '
        Write-Info "rename-session -t $XZ $B answered [$out]"
        Start-Sleep -Milliseconds 1000
        $names = ((P list-sessions -F '#{session_name}') | Sort-Object) -join ' '
        Check ($names -eq ((@($B, $XZ) | Sort-Object) -join ' ')) "both sessions keep their names" "list-sessions reads [$names]"
        Check-SessionStar $XZ
        Check-Buffers "prefix =" "^b{SLEEP:500}="
    } finally {
        P kill-server | Out-Null
        Start-Sleep -Milliseconds 800
        try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    }
}

Run-Scenario "default namespace" $null
Run-Scenario "-L namespace" ("rsi" + [guid]::NewGuid().ToString('N').Substring(0, 6))

Remove-PsmuxIsolatedConfig
Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue
Start-Sleep -Milliseconds 500
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
