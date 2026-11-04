# Issue #474: Git Bash / MSYS2 / zsh as default-shell backends on Windows.
#
# Covers the four confirmed defects end to end:
#   A. POSIX shells as default-shell spawn as LOGIN shells (PATH set up,
#      /usr/bin reachable under MSYS2, cwd preserved via CHERE_INVOKING).
#   B. git-bash.exe (GUI launcher) as default-shell is remapped to the real
#      bin\bash.exe: pane works, NO stray mintty/git-bash windows.
#   C. Reaper safety: a psmux CLI invocation with an MSYS2-style environment
#      (USERPROFILE unset) must NOT kill live servers (it used to slaughter
#      every server older than 10s machine-wide).
#   D. Shell matrix: bash (Git bin/usr), sh, MSYS2 bash, MSYS2 zsh all work
#      for session create + echo round-trip + split-window.
#
# Skips individual shells that are not installed.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:TestsSkipped++ }

function Wait-SessionAlive {
    param([string]$Name, [int]$TimeoutMs = 15000)
    $pf = "$psmuxDir\$Name.port"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $pf) {
            $port = (Get-Content $pf -Raw -EA SilentlyContinue).Trim()
            if ($port -match '^\d+$') {
                try { $t = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $t.Close(); return $true } catch {}
            }
        }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Start-ShellSession {
    param([string]$Name, [string]$ShellPath)
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$psmuxDir\$Name.*" -Force -EA SilentlyContinue
    $conf = "$env:TEMP\i474_$Name.conf"
    "set -g default-shell `"$ShellPath`"" | Set-Content -Path $conf -Encoding UTF8
    $env:PSMUX_CONFIG_FILE = $conf
    $env:PSMUX_NO_WARM = "1"
    Start-Process -FilePath $PSMUX -ArgumentList "new-session","-d","-s",$Name -WindowStyle Hidden
    $env:PSMUX_CONFIG_FILE = $null
    $env:PSMUX_NO_WARM = $null
    return (Wait-SessionAlive -Name $Name)
}

Write-Host "`n=== Issue #474 Tests ===" -ForegroundColor Cyan

# --- Part A: login shell + PATH + cwd -----------------------------------------
Write-Host "`n[Part A] POSIX default-shell spawns as login shell" -ForegroundColor Yellow
$msysBash = "C:\msys64\usr\bin\bash.exe"
if (Test-Path $msysBash) {
    $S = "i474_login"
    if (Start-ShellSession -Name $S -ShellPath $msysBash) {
        Start-Sleep -Seconds 5
        & $PSMUX send-keys -t $S 'shopt -q login_shell && echo ISLOGIN=yes || echo ISLOGIN=no' Enter
        Start-Sleep -Seconds 2
        & $PSMUX send-keys -t $S 'command -v uname >/dev/null && echo UNAME=found || echo UNAME=missing' Enter
        Start-Sleep -Seconds 2
        & $PSMUX send-keys -t $S 'pwd' Enter
        Start-Sleep -Seconds 2
        $cap = (& $PSMUX capture-pane -t $S -p 2>&1 | Out-String)
        if ($cap -match "ISLOGIN=yes") { Write-Pass "MSYS2 bash runs as login shell" }
        else { Write-Fail "MSYS2 bash not a login shell. capture: $($cap -split "`n" | Select-Object -Last 6)" }
        if ($cap -match "UNAME=found") { Write-Pass "/usr/bin in PATH (uname found)" }
        else { Write-Fail "uname not found in login shell PATH" }
        # CHERE_INVOKING: login shell must NOT cd to home; server cwd is not home
        if ($cap -notmatch "\`$ pwd\s*`n\s*/home/") { Write-Pass "cwd not hijacked to MSYS home" }
        else { Write-Fail "login shell cd'd to /home (CHERE_INVOKING missing)" }
        & $PSMUX kill-session -t $S 2>&1 | Out-Null
    } else { Write-Fail "MSYS2 bash session did not start" }
} else { Write-Skip "MSYS2 not installed" }

# --- Part B: git-bash.exe launcher remap ---------------------------------------
Write-Host "`n[Part B] git-bash.exe default-shell remaps to real bash" -ForegroundColor Yellow
$gitBash = "C:\Program Files\Git\git-bash.exe"
if (Test-Path $gitBash) {
    $minttyBefore = @(Get-Process mintty -EA SilentlyContinue).Count
    $S = "i474_launcher"
    if (Start-ShellSession -Name $S -ShellPath $gitBash) {
        Start-Sleep -Seconds 5
        $marker = "M474L_" + (Get-Random -Maximum 99999)
        & $PSMUX send-keys -t $S "echo $marker" Enter
        Start-Sleep -Seconds 3
        $cap = (& $PSMUX capture-pane -t $S -p 2>&1 | Out-String)
        if ($cap -match $marker) { Write-Pass "pane echo works (real bash, not launcher)" }
        else { Write-Fail "pane dead with git-bash.exe configured" }
        $pcc = (& $PSMUX display-message -t $S -p '#{pane_current_command}' 2>&1 | Out-String).Trim()
        if ($pcc -match "bash") { Write-Pass "pane_current_command is bash (got: $pcc)" }
        else { Write-Fail "pane_current_command expected bash, got: $pcc" }
        $minttyAfter = @(Get-Process mintty -EA SilentlyContinue).Count
        if ($minttyAfter -le $minttyBefore) { Write-Pass "no stray mintty windows spawned" }
        else { Write-Fail "stray mintty windows: before=$minttyBefore after=$minttyAfter" }
        & $PSMUX kill-session -t $S 2>&1 | Out-Null
        Get-Process mintty,git-bash -EA SilentlyContinue | Where-Object { $_.StartTime -gt (Get-Date).AddMinutes(-2) } | Stop-Process -Force -EA SilentlyContinue
    } else { Write-Fail "git-bash.exe session did not start" }
} else { Write-Skip "Git for Windows not installed" }

# --- Part C: reaper does not slaughter under MSYS2-style env -------------------
Write-Host "`n[Part C] MSYS2-style env CLI must not kill live servers" -ForegroundColor Yellow
$S = "i474_reapsafe"
& $PSMUX kill-session -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
& $PSMUX new-session -d -s $S
if (Wait-SessionAlive -Name $S) {
    Write-Host "  aging server past the 10s orphan-reap grace window..." -ForegroundColor DarkGray
    Start-Sleep -Seconds 12
    # Reproduce the MSYS2 login-shell environment: USERPROFILE unset, HOME POSIX.
    $psCmd = '$env:USERPROFILE=$null; $env:HOME="/home/nobody"; & "' + $PSMUX + '" list-sessions 2>&1 | Out-String'
    $out = pwsh -NoProfile -Command $psCmd
    Start-Sleep -Seconds 4
    & $PSMUX has-session -t $S 2>$null
    if ($LASTEXITCODE -eq 0) { Write-Pass "server survived MSYS2-style env invocation" }
    else { Write-Fail "REGRESSION: server killed by MSYS2-style env invocation" }
    # Bonus: with the profile-API fallback the sessions are even VISIBLE there.
    if ($out -match $S) { Write-Pass "sessions visible from MSYS2-style env (profile API fallback)" }
    else { Write-Fail "sessions not listed under MSYS2-style env: $out" }
    & $PSMUX kill-session -t $S 2>&1 | Out-Null
} else { Write-Fail "reap-safety session did not start" }

# --- Part D: shell matrix ------------------------------------------------------
Write-Host "`n[Part D] Shell matrix: create + echo + split" -ForegroundColor Yellow
$shells = @(
    @{ Label = "gitbinbash"; Path = "C:\Program Files\Git\bin\bash.exe" },
    @{ Label = "gitsh";      Path = "C:\Program Files\Git\bin\sh.exe" },
    @{ Label = "msysbash";   Path = "C:\msys64\usr\bin\bash.exe" },
    @{ Label = "msyszsh";    Path = "C:\msys64\usr\bin\zsh.exe" }
)
foreach ($sh in $shells) {
    if (-not (Test-Path $sh.Path)) { Write-Skip "$($sh.Label) ($($sh.Path))"; continue }
    $S = "i474_" + $sh.Label
    if (-not (Start-ShellSession -Name $S -ShellPath $sh.Path)) { Write-Fail "$($sh.Label): session did not start"; continue }
    Start-Sleep -Seconds 5
    $marker = "M474_" + (Get-Random -Maximum 99999)
    & $PSMUX send-keys -t $S "echo $marker" Enter
    Start-Sleep -Seconds 3
    $cap = (& $PSMUX capture-pane -t $S -p 2>&1 | Out-String)
    if ($cap -match $marker) { Write-Pass "$($sh.Label): echo round-trip" }
    else { Write-Fail "$($sh.Label): echo never appeared" }
    & $PSMUX split-window -v -t $S 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $panes = (& $PSMUX display-message -t $S -p '#{window_panes}' 2>&1 | Out-String).Trim()
    if ($panes -eq "2") { Write-Pass "$($sh.Label): split-window" }
    else { Write-Fail "$($sh.Label): expected 2 panes, got $panes" }
    & $PSMUX kill-session -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
}

# === Win32 TUI VISUAL VERIFICATION (mandatory layer) ===========================
Write-Host "`n[TUI] Visible window with bash default-shell" -ForegroundColor Yellow
$gitBinBash = "C:\Program Files\Git\bin\bash.exe"
if (Test-Path $gitBinBash) {
    $S = "i474_tui"
    & $PSMUX kill-session -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    $conf = "$env:TEMP\i474_tui.conf"
    "set -g default-shell `"$gitBinBash`"" | Set-Content -Path $conf -Encoding UTF8
    $env:PSMUX_CONFIG_FILE = $conf
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$S -PassThru
    $env:PSMUX_CONFIG_FILE = $null
    Start-Sleep -Seconds 6
    & $PSMUX has-session -t $S 2>$null
    if ($LASTEXITCODE -eq 0) { Write-Pass "TUI: attached session with bash default-shell" }
    else { Write-Fail "TUI: session did not come up" }
    & $PSMUX split-window -h -t $S 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $panes = (& $PSMUX display-message -t $S -p '#{window_panes}' 2>&1 | Out-String).Trim()
    if ($panes -eq "2") { Write-Pass "TUI: split-window created 2 bash panes" }
    else { Write-Fail "TUI: expected 2 panes, got $panes" }
    # The split's bash is new: Git bash takes 7 to 12 s to its first prompt
    # where every MSYS fork is slow, and keys sent before readline is up can
    # be lost. Wait for the prompt, then poll for the echo's OUTPUT line.
    for ($i = 0; $i -lt 60; $i++) {
        if ((& $PSMUX capture-pane -t $S -p 2>&1 | Out-String) -match '\$\s*$') { break }
        Start-Sleep -Milliseconds 500
    }
    $marker = "M474T_" + (Get-Random -Maximum 99999)
    & $PSMUX send-keys -t $S "echo $marker" Enter
    $cap = ""
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 500
        $cap = (& $PSMUX capture-pane -t $S -p 2>&1 | Out-String)
        if ($cap -match "(?m)^$marker") { break }
    }
    if ($cap -match "(?m)^$marker") { Write-Pass "TUI: echo round-trip in attached window" }
    else { Write-Fail "TUI: echo never appeared" }
    & $PSMUX kill-session -t $S 2>&1 | Out-Null
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
} else { Write-Skip "TUI check (Git bash missing)" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor DarkYellow
exit $script:TestsFailed
