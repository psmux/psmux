# switch-client -l, #{client_last_session}, the default `L` binding and the
# choose-tree session header, under -L and in the default namespace.
#
# Under -L ns every registry base reads `ns__name`. Three things leaked that
# base where tmux shows the session name:
#   - switch-client -l answered "no last session" although the client had just
#     switched, and #{client_last_session} read `ns__sesA`
#   - choose-tree drew `ns__sesA: 1 windows` as the session header
# and psmux had no default `L` binding at all, where tmux binds
# `L { switch-client -l }` (key-bindings.c).
#
# GROUND TRUTH: a real attached client in its own console. Keys go in through
# WriteConsoleInput (tests\injector.cs), the screen is read back with
# tests\conread.cs, and the client's session comes from list-clients.
#
# Isolation: everything runs in a PSMUX_DATA_DIR of its own (a temp dir unless
# one is set), the namespaced scenario under a -L name nobody else uses. A
# session in the default namespace of that data dir checks that nothing under
# -L ever lands there. Binary: $env:PSMUX_BIN, else psmux on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_BIN
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
$env:PSMUX_RUN_NOPAUSE = "1"

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_SWITCH_TO', 'PSMUX_CLIENT_LAST_SESSION', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}

$ownDir = -not $env:PSMUX_DATA_DIR
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_scl_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
if ($ownDir) { $env:PSMUX_DATA_DIR = Join-Path $TMP "data"; New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null }
$DATA = $env:PSMUX_DATA_DIR

Write-Host "`n=== switch-client -l and session names under -L ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $DATA"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $TMP "injector.exe"
$READ = Join-Path $TMP "conread.exe"
& $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs")  2>&1 | Out-Null
if (-not ((Test-Path $INJ) -and (Test-Path $READ))) { Write-Fail "could not build tests\injector.cs and tests\conread.cs"; exit 1 }

function Run-Scenario($label, $ns) {
    Write-Host "`n===== $label =====" -ForegroundColor Cyan
    $tag = [guid]::NewGuid().ToString('N').Substring(0, 4)
    $A = "la$tag"; $B = "lb$tag"; $C = "lc$tag"; $D = "ld$tag"
    $nsArgs = if ($ns) { @('-L', $ns) } else { @() }
    $base = { param($n) if ($ns) { "${ns}__$n" } else { $n } }

    function P { & $PSMUX @nsArgs @args 2>&1 }
    function PD { & $PSMUX @args 2>&1 }   # the default namespace of this data dir
    function Screen { return [string[]](& $READ $script:cpid) }
    function Send($keys) { & $INJ $script:cpid $keys | Out-Null; Start-Sleep -Milliseconds 900 }
    function Port($n) { $f = Join-Path $DATA ((& $base $n) + ".port"); if (Test-Path $f) { (Get-Content $f -Raw).Trim() } else { "" } }
    # The session the attached client is in, asked of every session it could be in.
    function Client-Session {
        foreach ($s in @($A, $B, $C)) {
            $c = ((P list-clients -t "$s" -F '#{client_session}') -join '').Trim()
            if ($c) { return $c }
        }
        if ($ns -and $ownDir) { $c = ((PD list-clients -t "$D" -F '#{client_session}') -join '').Trim(); if ($c) { return "default:$c" } }
        return ""
    }
    # Put the client in $to by a CLI switch from wherever it is, so a step
    # that follows never passes because the client had not moved before it.
    # The switch also makes the session it left the client's last session.
    function Move-To($to) {
        $now = Client-Session
        if ($now -ne $to -and $now) { Inside $now switch-client -t $to | Out-Null; [void](Wait-In $to) }
        return (Client-Session) -eq $to
    }
    function Wait-In($n) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 6000) { if ((Client-Session) -eq $n) { return $true }; Start-Sleep -Milliseconds 200 }
        return $false
    }
    # A command run from inside session $from, as a pane's shell would run it.
    function Inside($from) {
        $env:TMUX = "x,$(Port $from),0"
        try { $o = (P @args | Out-String).Trim(); $rc = $LASTEXITCODE } finally { Remove-Item Env:TMUX -EA SilentlyContinue }
        return @{ out = $o; rc = $rc }
    }

    P new-session -d -s $A | Out-Null; Start-Sleep -Milliseconds 800
    P new-session -d -s $B | Out-Null; Start-Sleep -Milliseconds 800
    P new-session -d -s $C | Out-Null; Start-Sleep -Milliseconds 800
    if ($ns -and $ownDir) { PD new-session -d -s $D | Out-Null; Start-Sleep -Milliseconds 800 }

    $p = Start-Process -FilePath $PSMUX -ArgumentList (@($nsArgs) + @('attach', '-t', $A)) -PassThru
    $script:cpid = $p.Id
    Start-Sleep -Seconds 4

    try {
        Write-Head "control"
        Check ((Client-Session) -eq $A) "the client is in $A" "the client is in [$(Client-Session)]"
        $r = Inside $A display-message -p '#{client_last_session}'
        Check ($r.out -eq "") "#{client_last_session} is empty before any switch" "#{client_last_session} read [$($r.out)] before any switch"

        Write-Head "switch-client -t $B, then -l from the CLI"
        Inside $A switch-client -t $B | Out-Null
        Check (Wait-In $B) "switch-client -t moved the client to $B" "the client is in [$(Client-Session)]"
        $r = Inside $B display-message -p '#{client_last_session}'
        Check ($r.out -eq $A) "#{client_last_session} reads $A" "#{client_last_session} read [$($r.out)]"
        $r = Inside $B switch-client -l
        Check ($r.rc -eq 0) "switch-client -l exits 0" "switch-client -l exited $($r.rc): [$($r.out)]"
        Check (Wait-In $A) "switch-client -l moved the client back to $A" "the client is in [$(Client-Session)]"

        # From $C with last $A, `-l` goes to $A and `-p` would go to $B, so a
        # key that ran -p instead of -l cannot pass.
        function From-C-Last-A { (Move-To $A) -and (Move-To $C) }

        Write-Head "switch-client -l from the prompt"
        Check (From-C-Last-A) "start in $C, last $A" "could not put the client in $C after $A"
        Send "^b{SLEEP:500}:{SLEEP:400}switch-client -l{ENTER}"
        Check (Wait-In $A) "the prompt's switch-client -l moved the client to $A" "the client is in [$(Client-Session)]"

        Write-Head "prefix L, the default binding"
        $keys = (P list-keys -T prefix) -join "`n"
        Check ($keys -cmatch '(?m)^bind-key\s+-T prefix\s+L\s+switch-client -l\s*$') "list-keys shows bind-key -T prefix L switch-client -l" `
            "list-keys has no L binding: [$((P list-keys -T prefix) -cmatch ' L ' -join ' | ')]"
        Check (From-C-Last-A) "start in $C, last $A" "could not put the client in $C after $A"
        Send "^b{SLEEP:500}L"
        Check (Wait-In $A) "prefix L moved the client to $A" "the client is in [$(Client-Session)]"

        Write-Head "a user binding of switch-client -l"
        # Bound on the server the key is pressed in (one server per session).
        Inside $C bind-key M switch-client -l | Out-Null
        Start-Sleep -Milliseconds 600
        Check (From-C-Last-A) "start in $C, last $A" "could not put the client in $C after $A"
        Send "^b{SLEEP:500}M"
        Check (Wait-In $A) "prefix M (bound to switch-client -l) moved the client to $A" "the client is in [$(Client-Session)]"

        Write-Head "unbind L takes the default away"
        Inside $C unbind-key L | Out-Null
        Start-Sleep -Milliseconds 600
        $keys = (Inside $C list-keys -T prefix).out
        Check (-not ($keys -cmatch '(?m)^bind-key\s+-T prefix\s+L\s')) "list-keys has no L after unbind-key L" "L is still listed after unbind-key L"
        Check (From-C-Last-A) "start in $C, last $A" "could not put the client in $C after $A"
        Send "^b{SLEEP:500}L"
        Start-Sleep -Milliseconds 1500
        Check ((Client-Session) -eq $C) "prefix L does nothing once unbound" "prefix L moved the client to [$(Client-Session)]"

        Write-Head "switch-client -n stays in the namespace"
        Check (Move-To $A) "start in $A" "could not put the client in $A"
        Inside $A switch-client -n | Out-Null
        Check (Wait-In $B) "switch-client -n moved the client to $B" "the client is in [$(Client-Session)]"

        Write-Head "choose-tree session headers"
        Send "^b{SLEEP:500}w"
        $sc = Screen
        $hdr = $sc | Where-Object { $_ -match "\b$([regex]::Escape($A)): \d+ windows" } | Select-Object -First 1
        Write-Info "header row: [$($hdr)]"
        Check ([bool]$hdr -and -not ($hdr -match '__')) "choose-tree shows the header as $A" "no header reads $A (row: [$hdr])"
        $leak = $sc | Where-Object { $ns -and $_.Contains("${ns}__") }
        Check (-not $leak) "choose-tree shows no ${ns}__ base" "choose-tree shows a base: [$($leak -join ' | ')]"
        Send "q"
    } finally {
        P kill-server | Out-Null
        if ($ns -and $ownDir) { PD kill-session -t $D | Out-Null }
        Start-Sleep -Milliseconds 800
        try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    }
}

Run-Scenario "-L namespace" ("scl" + [guid]::NewGuid().ToString('N').Substring(0, 6))
if ($ownDir) { Run-Scenario "default namespace (own data dir)" $null }

Remove-PsmuxIsolatedConfig
if ($ownDir) { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue }
Start-Sleep -Milliseconds 500
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
