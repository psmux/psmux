# test_issue600_bash_rehome.ps1 - the warm-pane rehome must speak the pane's shell
#
# Issue #600 "Failure to properly set current directory": with `default-shell`
# pointing at Git Bash, every warm-pane consume that carried a start directory
# typed the PowerShell rehome snippet into bash:
#
#   $  cd 'C:\Users\UserName1'; try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {}; cls
#   bash: syntax error near unexpected token `('
#
# The snippet was picked by the host OS rather than by the shell running in the
# pane, so bash and cmd.exe both got PowerShell. Two things went wrong at once:
# stray text was left on screen, and the `cd` never ran, so the pane stayed in
# the client's directory instead of the requested one.
#
# This test asserts BOTH halves on every affected path:
#   1. new-window -c <dir>      with default-shell = Git Bash
#   2. split-window -c <dir>    with default-shell = Git Bash
#   3. warm server claimed from a different cwd, with default-shell = Git Bash
#   4. cmd.exe as default-shell (a different wrong-dialect failure)
#   5. pwsh as default-shell (regression guard - it must keep working)
#
# tmux parity: tmux never types anything into a shell to set its directory; it
# chdir()s in the forked child before exec (spawn.c). psmux transplants an
# already-running warm shell, so it has to inject a line; the parity that is
# testable is the observable outcome - the pane IS in the requested directory
# and nothing stray is on screen.
#
# SAFETY: fully isolated. Runs with PSMUX_DATA_DIR pointed at a throwaway temp
# root, so its servers, port files, warm pool and single-server mutex cannot
# collide with a live psmux. Cleans up with kill-session / kill-server scoped to
# that data root - no name-based process kills.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_issue600"

$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

# ---- locate Git Bash; without it there is nothing to test -------------------
$BASH = @(
    "C:\Program Files\Git\bin\bash.exe",
    "C:\Program Files\Git\usr\bin\bash.exe",
    "C:\Program Files (x86)\Git\bin\bash.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $BASH) {
    $g = Get-Command bash.exe -EA SilentlyContinue
    if ($g -and $g.Source -notlike "*\WindowsApps\*") { $BASH = $g.Source }
}
if (-not $BASH) {
    Write-Host "  [SKIP] Git Bash not installed - #600 needs a POSIX shell to test" -ForegroundColor Yellow
    exit 0
}
Write-Host "  Using bash: $BASH" -ForegroundColor DarkGray

$CMDEXE = Join-Path $env:SystemRoot "System32\cmd.exe"
$PWSHEXE = (Get-Command pwsh -EA SilentlyContinue).Source

# ---- isolated data root + fixture dirs --------------------------------------
$TAG = [guid]::NewGuid().ToString('N').Substring(0, 8)
# NOT under %TEMP%. Storage Sense deletes EMPTY directories under %TEMP% (seen:
# a 90 minute old canary lost dirA, dirB and 'target dir' at 12:25:17 while its
# file survived)
# when the disk runs low; in sweep 2026-10-01_01-53-14 it ran at 06:15:28
# (Storsvc event 1003) between sections 2 and 3, took dirA, dirB and
# "target dir", and every later -c fell back to the client's cwd, which read
# as three product failures. A LocalAppData folder is outside its reach.
$FixtureBase = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "psmux-test-fixtures" } else { $env:TEMP }
$ROOT = Join-Path $FixtureBase "psmux-i600-$TAG"
$DIRA = Join-Path $ROOT "dirA"
$DIRB = Join-Path $ROOT "dirB"
$TARGET = Join-Path $ROOT "target dir"     # a space, so quoting is exercised
New-Item -ItemType Directory -Force -Path $ROOT, $DIRA, $DIRB, $TARGET | Out-Null
$CONF = Join-Path $ROOT "i600.conf"

$savedData = $env:PSMUX_DATA_DIR
$savedConf = $env:PSMUX_CONFIG_FILE
$savedSess = $env:PSMUX_SESSION
$savedSessName = $env:PSMUX_SESSION_NAME
$savedTgt = $env:PSMUX_TARGET_SESSION
$savedTmux = $env:TMUX

# The rehome is injected once, at a fresh prompt. Give the warm pane time to
# finish loading its shell before it is consumed, or the injected line lands
# before the prompt exists and the test measures a race, not the fix.
$WarmLoadMs = 5000

function Set-DefaultShell($shellPath) {
    Set-Content -Path $CONF -Value ("set -g default-shell `"$shellPath`"" + "`n") -Encoding ascii
}

function Capture($target) {
    return ((& $PSMUX capture-pane -p -t $target 2>&1) -join "`n")
}

# Wait until the pane has painted something that looks like a shell prompt.
function Wait-Prompt($target, $pattern, [int]$TimeoutMs = 25000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $o = Capture $target
        if ($o -match $pattern) { return $o }
        Start-Sleep -Milliseconds 150
    }
    return (Capture $target)
}

# The two halves of #600, asserted together on one pane.
#   - no stray text: the injected line must not be visible, in any dialect
#   - right directory: ask the shell itself where it is
function Assert-CleanRehome($label, $target, $expectedDir) {
    $cap = Capture $target
    Write-Info "$label capture: $((($cap -split "`n") | Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | ')"

    $dirty = @()
    if ($cap -match 'syntax error')        { $dirty += "bash syntax error" }
    if ($cap -match 'SetCurrentDirectory') { $dirty += "PowerShell rehome snippet echoed" }
    if ($cap -match 'cannot find the path'){ $dirty += "cmd could not parse the rehome" }
    if ($cap -match 'not recognized as an internal') { $dirty += "cmd rejected the rehome" }
    if ($dirty.Count -eq 0) {
        Write-Pass "$label leaves no stray text in the pane"
    } else {
        Write-Fail "$label left stray text: $($dirty -join ', ')"
        Write-Host ($cap.TrimEnd()) -ForegroundColor DarkYellow
    }
    return $cap
}

# Ask a bash pane for its own cwd in Windows form and compare with the request.
function Assert-BashPwd($label, $target, $expectedDir) {
    $marker = "I600PWD"
    & $PSMUX send-keys -t $target "echo $marker`$(pwd -W)" Enter 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(15)
    $seen = ""
    # The echoed command line also carries the marker, but there it is followed
    # by `$(pwd -W)`; the answer line is the one where a drive letter follows.
    # The directory may contain spaces, so match to end of line, not to space.
    $answer = "(?m)^\s*$marker([A-Za-z]:.*?)\s*$"
    while ((Get-Date) -lt $deadline) {
        $seen = Capture $target
        if ($seen -match $answer) { break }
        Start-Sleep -Milliseconds 250
    }
    if ($seen -match $answer) {
        $actual = $Matches[1]
        $norm = { param($p) ($p -replace '\\', '/').TrimEnd('/').ToLowerInvariant() }
        $a = & $norm $actual
        $e = & $norm $expectedDir
        if ($a -eq $e) {
            Write-Pass "$label pane pwd is the requested dir ($actual)"
        } else {
            Write-Fail "$label pane pwd is '$actual', expected '$expectedDir'"
        }
    } else {
        Write-Fail "$label could not read pwd back from the bash pane"
        Write-Host ($seen.TrimEnd()) -ForegroundColor DarkYellow
    }
}

function Kill-Session($name) { & $PSMUX kill-session -t $name 2>&1 | Out-Null }

# A -c or cwd assertion is only about psmux while the directory it names still
# exists. If something outside psmux removed a fixture, say so with evidence
# and do not judge psmux on a directory that is not there.
function Test-Fixtures($label, [string[]]$dirs) {
    $gone = @($dirs | Where-Object { -not (Test-Path -LiteralPath $_) })
    if ($gone.Count -eq 0) { return $true }
    $ss = ''
    try {
        $ev = @(Get-WinEvent -FilterHashtable @{ LogName = 'Microsoft-Windows-Storsvc/Diagnostic'; Id = 1003; StartTime = $script:SuiteStart } -ErrorAction Stop)
        if ($ev.Count) { $ss = " Storage Sense ran at " + (($ev | ForEach-Object { $_.TimeCreated.ToString('HH:mm:ss.fff') }) -join ',') + "." }
    } catch { }
    Write-Fail "$label not judged: fixture dir(s) deleted by something outside psmux before this step: $($gone -join '; ').$ss"
    return $false
}
$script:SuiteStart = Get-Date

try {
    $env:PSMUX_DATA_DIR = $ROOT
    $env:PSMUX_CONFIG_FILE = $CONF
    Remove-Item Env:\PSMUX_SESSION, Env:\PSMUX_SESSION_NAME, Env:\PSMUX_TARGET_SESSION, Env:\TMUX -EA SilentlyContinue

    # =====================================================================
    Write-Host "`n--- 1/2. Git Bash default-shell: new-window -c and split-window -c ---" -ForegroundColor Cyan
    Set-DefaultShell $BASH
    Kill-Session "${SESSION}_bash"
    & $PSMUX new-session -d -s "${SESSION}_bash" -x 120 -y 30 2>&1 | Out-Null
    # The one cold Git Bash login of this suite (later sessions claim a warm
    # server whose shell is already up). Its git prompt costs about 14 MSYS
    # forks, and a fork can take 150 ms on a busy machine with no psmux in the
    # picture at all, so a login of 15 to 23 s is the shell, not psmux.
    $base = Wait-Prompt "${SESSION}_bash" '\$ ' -TimeoutMs 60000
    if ($base -match '\$') { Write-Pass "bash session created" } else { Write-Fail "bash session did not start: $base" }
    Start-Sleep -Milliseconds $WarmLoadMs   # let the warm pane finish loading

    & $PSMUX new-window -t "${SESSION}_bash" -c $TARGET 2>&1 | Out-Null
    Wait-Prompt "${SESSION}_bash:1" '\$ ' | Out-Null
    Start-Sleep -Milliseconds 1500          # let the injected rehome settle
    Assert-CleanRehome "new-window -c" "${SESSION}_bash:1" $TARGET | Out-Null
    if (Test-Fixtures "new-window -c pwd" @($TARGET)) { Assert-BashPwd "new-window -c" "${SESSION}_bash:1" $TARGET }

    Start-Sleep -Milliseconds $WarmLoadMs   # let the replenished warm pane load
    & $PSMUX split-window -t "${SESSION}_bash:1" -c $TARGET 2>&1 | Out-Null
    Wait-Prompt "${SESSION}_bash:1.1" '\$ ' | Out-Null
    Start-Sleep -Milliseconds 1500
    Assert-CleanRehome "split-window -c" "${SESSION}_bash:1.1" $TARGET | Out-Null
    if (Test-Fixtures "split-window -c pwd" @($TARGET)) { Assert-BashPwd "split-window -c" "${SESSION}_bash:1.1" $TARGET }

    Kill-Session "${SESSION}_bash"

    # =====================================================================
    Write-Host "`n--- 3. Git Bash default-shell: warm server claimed from another cwd ---" -ForegroundColor Cyan
    # A warm server pre-spawns its shell in the directory it was started from.
    # Claiming it from elsewhere re-homes that shell - the third injection site.
    Kill-Session "${SESSION}_warm"
    Push-Location $DIRA
    & $PSMUX warmup 2>&1 | Out-Null
    Pop-Location
    Start-Sleep -Milliseconds $WarmLoadMs

    Push-Location $DIRB
    & $PSMUX new-session -d -s "${SESSION}_warm" -x 120 -y 30 2>&1 | Out-Null
    Pop-Location
    Wait-Prompt "${SESSION}_warm" '\$ ' | Out-Null
    Start-Sleep -Milliseconds 1500
    Assert-CleanRehome "warm claim" "${SESSION}_warm" $DIRB | Out-Null
    if (Test-Fixtures "warm claim pwd" @($DIRA, $DIRB)) { Assert-BashPwd "warm claim" "${SESSION}_warm" $DIRB }
    Kill-Session "${SESSION}_warm"

    # =====================================================================
    Write-Host "`n--- 4. cmd.exe default-shell: new-window -c ---" -ForegroundColor Cyan
    # cmd chains on `&`, not `;`, and has no `try {}` - the PowerShell snippet
    # made it print "The system cannot find the path specified." and stay put.
    Set-DefaultShell $CMDEXE
    Kill-Session "${SESSION}_cmd"
    & $PSMUX new-session -d -s "${SESSION}_cmd" -x 120 -y 30 2>&1 | Out-Null
    Wait-Prompt "${SESSION}_cmd" '[A-Za-z]:\\.*>' | Out-Null
    Start-Sleep -Milliseconds $WarmLoadMs

    & $PSMUX new-window -t "${SESSION}_cmd" -c $TARGET 2>&1 | Out-Null
    Wait-Prompt "${SESSION}_cmd:1" '[A-Za-z]:\\.*>' | Out-Null
    Start-Sleep -Milliseconds 1500
    $ccap = Assert-CleanRehome "cmd new-window -c" "${SESSION}_cmd:1" $TARGET
    # cmd's prompt IS its cwd, so the prompt is the directory assertion.
    if (-not (Test-Fixtures "cmd new-window -c prompt" @($TARGET))) { }
    elseif ($ccap -match [regex]::Escape($TARGET)) {
        Write-Pass "cmd new-window -c pane prompt is the requested dir"
    } else {
        Write-Fail "cmd new-window -c pane prompt is not '$TARGET'"
        Write-Host ($ccap.TrimEnd()) -ForegroundColor DarkYellow
    }
    Kill-Session "${SESSION}_cmd"

    # =====================================================================
    Write-Host "`n--- 5. pwsh default-shell: regression guard ---" -ForegroundColor Cyan
    if (-not $PWSHEXE) {
        Write-Host "  [SKIP] pwsh not on PATH" -ForegroundColor Yellow
    } else {
        # Bare name on purpose: psmux resolves it through PATH. Writing the full
        # WindowsApps path unquoted would split on its spaces.
        Set-DefaultShell "pwsh -NoProfile"
        Kill-Session "${SESSION}_pwsh"
        & $PSMUX new-session -d -s "${SESSION}_pwsh" -x 120 -y 30 2>&1 | Out-Null
        Wait-Prompt "${SESSION}_pwsh" 'PS [A-Za-z]:\\' | Out-Null
        Start-Sleep -Milliseconds $WarmLoadMs

        & $PSMUX new-window -t "${SESSION}_pwsh" -c $TARGET 2>&1 | Out-Null
        Wait-Prompt "${SESSION}_pwsh:1" 'PS [A-Za-z]:\\' | Out-Null
        Start-Sleep -Milliseconds 1500
        $pcap = Assert-CleanRehome "pwsh new-window -c" "${SESSION}_pwsh:1" $TARGET
        if (-not (Test-Fixtures "pwsh new-window -c prompt" @($TARGET))) { }
        elseif ($pcap -match [regex]::Escape($TARGET)) {
            Write-Pass "pwsh new-window -c pane prompt is the requested dir (unchanged behaviour)"
        } else {
            Write-Fail "pwsh new-window -c pane prompt is not '$TARGET'"
            Write-Host ($pcap.TrimEnd()) -ForegroundColor DarkYellow
        }
        Kill-Session "${SESSION}_pwsh"
    }
}
finally {
    foreach ($s in @("${SESSION}_bash", "${SESSION}_warm", "${SESSION}_cmd", "${SESSION}_pwsh")) {
        & $PSMUX kill-session -t $s 2>&1 | Out-Null
    }
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    $env:PSMUX_DATA_DIR = $savedData
    $env:PSMUX_CONFIG_FILE = $savedConf
    if ($null -ne $savedSess)     { $env:PSMUX_SESSION = $savedSess }
    if ($null -ne $savedSessName) { $env:PSMUX_SESSION_NAME = $savedSessName }
    if ($null -ne $savedTgt)      { $env:PSMUX_TARGET_SESSION = $savedTgt }
    if ($null -ne $savedTmux)     { $env:TMUX = $savedTmux }
    Remove-Item -Recurse -Force $ROOT -EA SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed Failed: $script:TestsFailed"
exit $script:TestsFailed
