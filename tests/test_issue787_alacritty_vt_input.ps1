# Issue #787: a client started in Alacritty on Windows reads its input on the
# VT input path, as one started in WezTerm does.
#
# Alacritty writes keys to its ConPTY as VT bytes and has no win32-input-mode,
# but nothing put its clients on the VT input path, so they read through
# conhost's key translation. Measured under Alacritty 0.17.0 on Windows 11
# 26200 with PSMUX_INPUT_DEBUG=1, two things broke there:
#
#   * Ctrl+J (0x0a) came out of conhost as Enter with CONTROL, went to the pane
#     as `send-key C-Enter`, and a C-j prefix never armed. C-b worked. The VT
#     parser reads 0x0a as C-j (#642).
#   * At startup the input pump read `5`, `5` and Alt+\ and typed them into the
#     pane: the torn tail of a colour reply that landed after the colour query
#     drain had returned. The drain waits for late replies only on the VT input
#     path (#646, settle_window).
#
# Alacritty sets no TERM_PROGRAM. The variable it always sets on Windows is
# ALACRITTY_LOG (alacritty/src/logging.rs:204, v0.17.0), so that is what
# needs_vt_input() now looks for.
#
# PART A runs the client under a real pseudoconsole, kernel32's
# CreatePseudoConsole as Alacritty uses it, with ALACRITTY_LOG set as an
# Alacritty window sets it, and writes raw bytes into the input pipe the way
# Alacritty writes a key (tests\conpty_vt_keys.cs). On the console route conhost
# turns 0x0a into Ctrl+Enter there, exactly as in Alacritty.
# PART B starts the client in a real Alacritty window and reads the pane for
# the reply tail. It is skipped when Alacritty is not installed. It opens a
# window, so do not type while it runs. Alacritty loads conpty.dll from PATH
# when it finds one (WezTerm's install directory has one), and then runs the
# client under that OpenConsole.exe instead of the system conhost. The torn
# reply tail was measured under WezTerm's OpenConsole (16 of 17 starts on master)
# and not under the system conhost on 26200 (0 of 4); the Ctrl+J failure shows
# under both.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:TestsSkipped = 0
$script:OpenedPids = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$tag = [guid]::NewGuid().ToString('N').Substring(0, 6)
$NS_IN  = "ala-in-$tag";  $S_IN  = "alain"
$NS_WIN = "ala-win-$tag"; $S_WIN = "alawin"
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ala_" + $tag)
New-Item -ItemType Directory -Force $TMP | Out-Null
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
# No profile, so the prompt is the stock `PS <path>>` whatever the user's
# profile draws.
$SHELL_LINE = "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'"

function Write-Conf($name, $lines) {
    $path = Join-Path $TMP "$name.conf"
    (($lines -join "`n") + "`n") | Set-Content $path -Encoding ASCII
    return $path
}

function Kill-Ns($ns) {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    Get-ChildItem "$psmuxDir\${ns}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

function Stop-Opened {
    foreach ($id in $script:OpenedPids) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:OpenedPids = @()
}

# What every client under test must not inherit from the shell that runs this
# suite: a psmux session around it would make it refuse to nest or skip the
# colour query, and another terminal's signal would decide its input path.
$scrub = "Remove-Item Env:\PSMUX_SESSION,Env:\PSMUX_SESSION_NAME,Env:\PSMUX_PANE," +
         "Env:\PSMUX_TARGET_SESSION,Env:\TMUX,Env:\TMUX_PANE,Env:\PSMUX_VT_INPUT," +
         "Env:\TERM_PROGRAM,Env:\WEZTERM_PANE,Env:\TERMINAL_EMULATOR -EA SilentlyContinue"

# ==========================================================================
# PART A: raw bytes on a pseudoconsole, with Alacritty's variable set
# ==========================================================================

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
       Select-Object -First 1 -ExpandProperty FullName
$VTHOST = Join-Path $TMP "conpty_vt_keys.exe"
if ($csc) { & $csc /nologo /optimize /out:$VTHOST (Join-Path $PSScriptRoot "conpty_vt_keys.cs") 2>&1 | Out-Null }

# The variables a client under test must not take from the shell running this
# suite, removed from this process for as long as it takes to start the host,
# which the client inherits from.
$clientVars = @("PSMUX_SESSION", "PSMUX_SESSION_NAME", "PSMUX_PANE", "PSMUX_TARGET_SESSION",
                "TMUX", "TMUX_PANE", "PSMUX_VT_INPUT", "TERM_PROGRAM", "WEZTERM_PANE",
                "TERMINAL_EMULATOR", "ALACRITTY_LOG")

# Start the client under the host, which waits for it to come up, writes
# `$keys` (hex bytes, comma separated) 700 ms apart, and holds the console open
# long enough for the panes to be counted.
function Run-Keys($conf, $keys, [bool]$alacritty) {
    Kill-Ns $NS_IN
    # Removed through the Env: drive, not [Environment]::SetEnvironmentVariable
    # with $null: under PowerShell 7.6 a child started by Start-Process after
    # that call still gets the variable, set to the empty string, and an empty
    # WEZTERM_PANE or ALACRITTY_LOG is still present, so every client here took
    # the VT input route whatever the build.
    $saved = @{}
    foreach ($k in $clientVars) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k)
        Remove-Item "Env:\$k" -EA SilentlyContinue
    }
    if ($alacritty) { Set-Item "Env:\ALACRITTY_LOG" "C:\Temp\Alacritty-4242.log" }
    $savedWarm = $env:PSMUX_NO_WARM; $env:PSMUX_NO_WARM = "1"
    $steps = "w7000," + (($keys -split ',') -join ",w700,") + ",h7000"
    $p = Start-Process -FilePath $VTHOST -WindowStyle Hidden -PassThru `
        -ArgumentList @($steps, "`"$PSMUX`"", "-L", $NS_IN, "-f", "`"$conf`"", "new-session", "-s", $S_IN)
    $script:OpenedPids += $p.Id
    foreach ($k in $clientVars) {
        if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -EA SilentlyContinue }
    }
    $env:PSMUX_NO_WARM = $savedWarm
    # Count while the host holds the console: 7 s to come up, the keys, then 2 s.
    Start-Sleep -Milliseconds (7000 + 700 * ($keys -split ',').Count + 2000)
    $n = Inner-Panes
    $p | Wait-Process -Timeout 15 -EA SilentlyContinue
    Kill-Ns $NS_IN
    return $n
}

function Inner-Panes {
    $v = (& $PSMUX -L $NS_IN display-message -t $S_IN -p '#{window_panes}' 2>&1 | Out-String).Trim()
    if ($v -match '^\d+$') { return [int]$v } else { return -1 }
}

$confCj = Write-Conf "cj" @("set -g prefix C-j", "bind c split-window", $SHELL_LINE, "set -g status-left ''")
$confCb = Write-Conf "cb" @("set -g prefix C-b", "bind c split-window", $SHELL_LINE, "set -g status-left ''")

if (-not (Test-Path $VTHOST)) {
    Write-Fail "could not build tests\conpty_vt_keys.cs (csc.exe unavailable)"
} else {
    Write-Head "A1. Ctrl+J (0x0a) arms a C-j prefix"
    $n = Run-Keys $confCj "0a,63" $true
    Check ($n -eq 2) "C-j then c split the window (panes $n)" "C-j then c left [$n] panes, so the prefix never armed"

    Write-Head "A2. Enter (0x0d) is not C-j"
    $n = Run-Keys $confCj "0d,63" $true
    Check ($n -eq 1) "Enter then c did not split (panes $n)" "Enter then c left [$n] panes"

    Write-Head "A3. Ctrl+B (0x02) arms a C-b prefix as before"
    $n = Run-Keys $confCb "02,63" $true
    Check ($n -eq 2) "C-b then c split the window (panes $n)" "C-b then c left [$n] panes"

    # The same 0x0a without Alacritty's variable must stay on the console route,
    # where conhost hands it over as Ctrl+Enter. It is the same on every build,
    # and it proves this host shows the problem at all: had a VT route signal
    # leaked into the client, A1 would pass on a build without the fix.
    Write-Head "A4. without ALACRITTY_LOG the client stays on the console route"
    $n = Run-Keys $confCj "0a,63" $false
    Check ($n -eq 1) "without ALACRITTY_LOG, C-j then c left one pane (the console route)" `
        "without ALACRITTY_LOG, C-j then c left [$n] panes, so a VT route signal reached the client"
}

# ==========================================================================
# PART B: a real Alacritty window, nothing typed into the pane at startup
# ==========================================================================

Write-Head "B. a real Alacritty window types nothing into the pane at startup"
$alacritty = $null
foreach ($cand in @("C:\Program Files\Alacritty\alacritty.exe", "$env:LOCALAPPDATA\Programs\Alacritty\alacritty.exe")) {
    if (Test-Path $cand) { $alacritty = $cand; break }
}
if (-not $alacritty) {
    $c = Get-Command alacritty -EA SilentlyContinue
    if ($c) { $alacritty = $c.Source }
}
if (-not $alacritty) {
    Write-Skip "Alacritty is not installed"
} else {
    Write-Info "alacritty: $alacritty"
    $confWin = Write-Conf "win" @($SHELL_LINE, "set -g status-left ''")
    # Alacritty sets ALACRITTY_LOG for the child itself, which is the point,
    # so the scrub leaves it alone and removes only what the shell running this
    # suite would otherwise hand down.
    $launch = Join-Path $TMP "launch.ps1"
    @(
        $scrub,
        "Remove-Item Env:\PSMUX_HOST_COLORS -EA SilentlyContinue",
        "`$env:PSMUX_NO_WARM = '1'",
        # A short start directory, so the prompt fits on one row of a default
        # sized window: a long one wraps, and the pane holds the wrapped rows
        # as separate lines, so the prompt line was never found.
        "Set-Location `"`$env:SystemDrive\`"",
        "& '$PSMUX' -L $NS_WIN -f '$confWin' new-session -s $S_WIN"
    ) | Set-Content -Path $launch -Encoding UTF8

    $starts = 3
    for ($n = 1; $n -le $starts; $n++) {
        Kill-Ns $NS_WIN
        $p = Start-Process -FilePath $alacritty -PassThru `
            -ArgumentList @("-e", $POWERSHELL, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $launch)
        $script:OpenedPids += $p.Id
        $up = $false
        for ($i = 0; $i -lt 60; $i++) {
            Start-Sleep -Milliseconds 300
            & $PSMUX -L $NS_WIN has-session -t $S_WIN 2>$null
            if ($LASTEXITCODE -eq 0) { $up = $true; break }
        }
        if (-not $up) { Write-Fail "start $n : the client never came up in Alacritty"; Stop-Opened; continue }
        # The prompt has to be drawn before anything typed shows after it.
        Start-Sleep -Seconds 5
        $cap = (& $PSMUX -L $NS_WIN capture-pane -t $S_WIN -p 2>&1 | Out-String)
        $prompt = ($cap -split "`n" | Where-Object { $_ -match '^PS .*>' } | Select-Object -Last 1)
        $typed = if ($prompt) { ($prompt -replace '^PS [^>]*>', '').Trim() } else { $null }
        Write-Info "start $n : prompt line [$prompt]"
        if ($null -eq $typed) {
            Write-Info "start $n : the pane held [$($cap.Trim())]"
            Write-Fail "start $n : no PowerShell prompt in the pane"
        } else {
            Check ($typed -eq "") "start $n : nothing was typed at the prompt" `
                "start $n : the pane received [$typed] at startup"
        }
        Kill-Ns $NS_WIN
        Start-Sleep -Milliseconds 800
        Stop-Opened
    }
}

# -- Done --

Kill-Ns $NS_IN; Kill-Ns $NS_WIN
Stop-Opened
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed  skipped: $script:TestsSkipped" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
