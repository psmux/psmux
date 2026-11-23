# test_issue630_conpty_cwd_pin.ps1
#
# Issue #630 "Split pane's ConPTY host pins source directory, blocking deletion"
#
# Splitting a pane used to leave the new pane's conhost.exe parked in the split's
# start directory for the pane's whole life. The directory could then not be
# deleted for as long as the pane lived, even after its shell had cd'd away:
#
#   Remove-Item: The process cannot access the file
#   'C:\Users\...\Temp\psmux-i630-446977\X' because it is being used by another
#   process.
#
# MEASURED cause. Reading RTL_USER_PROCESS_PARAMETERS.CurrentDirectory out of the
# PEB of every psmux descendant, on the unfixed binary, after `split-window -c X`
# and after the split pane's shell had moved to a neutral directory:
#
#   74632  psmux     ...\neutral
#   12120  conhost   ...\neutral      (pane 0's host, at the server's cwd)
#   24484  pwsh      ...\neutral
#   21756  conhost   ...\X       <==  the split pane's host, stuck in X
#   74528  pwsh      ...\neutral      (the split pane's shell, correctly moved)
#
# The server wrapped pane creation in env::set_current_dir(start_dir).
# CreatePseudoConsole() spawns the pane's conhost.exe from the server process, so
# the host inherited that directory and held a handle on it. A console host has
# no `cd`, so it never let go. The pane's shell was ALSO given the same directory
# explicitly (lpCurrentDirectory on CreateProcessW for a cold spawn, an injected
# cd for a warm transplant), so the server chdir was pure damage. It is gone.
#
# tmux parity: tmux chdir()s inside the forked child only, so the pane's own
# process is the single holder of the start directory and it releases it on the
# first cd. That is what is asserted below: the pane still STARTS in the
# requested directory, and once its shell leaves, nothing psmux owns keeps it.
#
# The matrix runs twice, because the two spawn paths differ completely:
#   warm  - the pane is transplanted from the pre-spawned standby pool and
#           re-homed with an injected cd (this path was already correct)
#   cold  - PSMUX_NO_WARM=1 forces a fresh ConPTY per pane (this is the path
#           that reproduced the bug)
#
# NOT asserted here, because it is deliberate and documented in src/server/mod.rs:
# the server adopts the SESSION's start directory as its own Win32 cwd and keeps
# it for life, so that new-window/split-window without -c inherit it. A session
# therefore holds its own start directory. That is the "host sits at the server's
# cwd" behaviour the reporter described as acceptable, and it pins one directory
# per server rather than one per split.
#
# SAFETY: fully isolated. Unique -L socket namespace, PSMUX_DATA_DIR pointed at a
# throwaway temp root, sessions killed by name, and any surviving server killed
# only when its executable path is the binary under test.

$ErrorActionPreference = "Continue"

. (Join-Path $PSScriptRoot 'psmux_test_helpers.ps1')
$PSMUX = Get-PsmuxExe -TestsRoot $PSScriptRoot

$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

Write-Host "Issue #630: a pane's ConPTY host must not pin the pane's start directory"
Write-Host "  binary: $PSMUX" -ForegroundColor DarkGray

# ---- isolated data root + fixture dirs --------------------------------------
$TAG = [guid]::NewGuid().ToString('N').Substring(0, 8)
$ROOT = Join-Path $env:TEMP "psmux-i630-$TAG"
$NEUTRAL = Join-Path $ROOT "neutral"     # the session lives here
$X = Join-Path $ROOT "X"                 # the directory under test
New-Item -ItemType Directory -Force -Path $ROOT, $NEUTRAL, $X | Out-Null

$savedData = $env:PSMUX_DATA_DIR
$savedWarm = $env:PSMUX_NO_WARM
$savedSess = $env:PSMUX_SESSION_NAME
$savedTarget = $env:PSMUX_TARGET_SESSION
$env:PSMUX_DATA_DIR = Join-Path $ROOT "data"
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_TARGET_SESSION = $null
New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null

$NS = "i630$TAG"
function P { & $PSMUX -L $NS @args 2>&1 }

function Reset-X {
    Remove-Item -LiteralPath $X -Recurse -Force -EA SilentlyContinue
    New-Item -ItemType Directory -Force -Path $X | Out-Null
}

function Get-PanePath($target) {
    ((P display-message -p -t $target '#{pane_current_path}') -join '').Trim()
}

# Wait until a pane's reported directory settles on $want.
function Wait-PanePath($target, $want, $timeoutMs = 10000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $p = Get-PanePath $target
        if ($p -and ($p.TrimEnd('\') -ieq $want.TrimEnd('\'))) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

# Move a pane's shell out of $X for real. PowerShell's Set-Location moves only
# the provider location, so the Win32 process directory (which is what actually
# holds the handle) is synced explicitly, the same way psmux's own rehome snippet
# does it (#600).
function Move-PaneOut($target) {
    P send-keys -t $target "Set-Location '$NEUTRAL'; [System.IO.Directory]::SetCurrentDirectory('$NEUTRAL')" Enter | Out-Null
    return (Wait-PanePath $target $NEUTRAL)
}

function Test-Delete($dir) {
    # Retry briefly: a shell that has just left the directory can still be
    # finishing its own bookkeeping. A genuine host pin never clears.
    $err = ""
    for ($i = 0; $i -lt 20; $i++) {
        try {
            Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction Stop
            return @{ ok = $true; err = "" }
        } catch {
            $err = $_.Exception.Message
            Start-Sleep -Milliseconds 200
        }
    }
    return @{ ok = $false; err = $err }
}

function Kill-Sess($name) {
    P kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
}

# =============================================================================
# The matrix, run once per spawn path.
# =============================================================================
foreach ($mode in @('warm', 'cold')) {
    if ($mode -eq 'cold') { $env:PSMUX_NO_WARM = "1" } else { $env:PSMUX_NO_WARM = $null }
    Write-Host "`n--- spawn path: $mode ---" -ForegroundColor Cyan

    # -- 1. split-window -c X: the reported case ------------------------------
    Reset-X
    $s = "i630a$mode"
    P new-session -d -s $s -c $NEUTRAL -x 80 -y 24 | Out-Null
    Start-Sleep -Milliseconds 1800
    P split-window -t "${s}:0.0" -c $X | Out-Null
    Start-Sleep -Milliseconds 2500

    $sp = Get-PanePath "${s}:0.1"
    if ($sp.TrimEnd('\') -ieq $X.TrimEnd('\')) {
        Write-Pass "$mode : split-window -c X still STARTS the pane in X"
    } else {
        Write-Fail "$mode : split-window -c X did not start the pane in X (got '$sp')"
    }

    if (Move-PaneOut "${s}:0.1") {
        $r = Test-Delete $X
        if ($r.ok) {
            Write-Pass "$mode : X is deletable while the split pane is still alive"
        } else {
            Write-Fail "$mode : X still pinned after the split shell left it. $($r.err)"
        }
    } else {
        Write-Fail "$mode : could not move the split pane's shell out of X (path=$(Get-PanePath "${s}:0.1"))"
    }
    Kill-Sess $s

    # -- 2. new-window -c X ---------------------------------------------------
    Reset-X
    $s = "i630b$mode"
    P new-session -d -s $s -c $NEUTRAL -x 80 -y 24 | Out-Null
    Start-Sleep -Milliseconds 1800
    P new-window -t $s -c $X | Out-Null
    Start-Sleep -Milliseconds 2500

    $wp = Get-PanePath "${s}:1.0"
    if ($wp.TrimEnd('\') -ieq $X.TrimEnd('\')) {
        Write-Pass "$mode : new-window -c X still STARTS the pane in X"
    } else {
        Write-Fail "$mode : new-window -c X did not start the pane in X (got '$wp')"
    }

    if (Move-PaneOut "${s}:1.0") {
        $r = Test-Delete $X
        if ($r.ok) {
            Write-Pass "$mode : X is deletable while the new-window pane is still alive"
        } else {
            Write-Fail "$mode : X still pinned after the new-window shell left it. $($r.err)"
        }
    } else {
        Write-Fail "$mode : could not move the new-window pane's shell out of X"
    }
    Kill-Sess $s

    # -- 3. control: a never-split pane that visits X and leaves --------------
    # The reporter's control case. A pane that merely cd's into X and back out
    # must never pin it; nothing about X was ever handed to psmux.
    Reset-X
    $s = "i630c$mode"
    P new-session -d -s $s -c $NEUTRAL -x 80 -y 24 | Out-Null
    Start-Sleep -Milliseconds 1800
    P send-keys -t "${s}:0.0" "Set-Location '$X'; [System.IO.Directory]::SetCurrentDirectory('$X')" Enter | Out-Null
    if (Wait-PanePath "${s}:0.0" $X) {
        Write-Info "control pane reached X"
    } else {
        Write-Info "control pane path is $(Get-PanePath "${s}:0.0")"
    }
    if (Move-PaneOut "${s}:0.0") {
        $r = Test-Delete $X
        if ($r.ok) {
            Write-Pass "$mode : control, a never-split pane that left X does not pin it"
        } else {
            Write-Fail "$mode : control, X pinned by a never-split pane. $($r.err)"
        }
    } else {
        Write-Fail "$mode : could not move the control pane's shell out of X"
    }
    Kill-Sess $s

    # -- 4. two splits in a row off the same source directory -----------------
    # Every extra pane used to add another host stuck in X. One surviving host
    # is enough to block the delete, so this catches a partial fix.
    Reset-X
    $s = "i630d$mode"
    P new-session -d -s $s -c $NEUTRAL -x 80 -y 24 | Out-Null
    Start-Sleep -Milliseconds 1800
    P split-window -t "${s}:0.0" -c $X | Out-Null
    Start-Sleep -Milliseconds 2200
    P split-window -t "${s}:0.1" -c $X | Out-Null
    Start-Sleep -Milliseconds 2500

    $moved = (Move-PaneOut "${s}:0.1") -and (Move-PaneOut "${s}:0.2")
    if ($moved) {
        $r = Test-Delete $X
        if ($r.ok) {
            Write-Pass "$mode : X is deletable with two live panes that started there"
        } else {
            Write-Fail "$mode : X still pinned with two panes that started there. $($r.err)"
        }
    } else {
        $why = foreach ($t in "${s}:0.1", "${s}:0.2") {
            $pp = ((P display-message -p -t $t '#{pane_pid}') -join '').Trim()
            $kids = if ($pp -match '^\d+$') { (Get-CimInstance Win32_Process -Filter "ParentProcessId=$pp" | ForEach-Object { "$($_.ProcessId):$($_.Name)" }) -join ' ' }
            "$t pid=$pp kids=[$kids] path='$(Get-PanePath $t)' screen='$(((P capture-pane -p -t $t) -join ' / ').Trim())'"
        }
        Write-Fail "$mode : could not move both split shells out of X ($($why -join ' ;; '))"
    }
    Kill-Sess $s
}

# =============================================================================
# 5. -c has to keep working on the COMMAND path too, not just the bare shell
#    path. This is the arm that would break if the server chdir had been the
#    thing carrying the start directory.
#
#    A command process started in X legitimately holds X while it runs, exactly
#    as tmux's forked child does, so the delete is only expected once the pane
#    is gone. What matters is that NOTHING ELSE is still holding it then.
# =============================================================================
$env:PSMUX_NO_WARM = "1"
Reset-X
$s = "i630e"
P new-session -d -s $s -c $NEUTRAL -x 80 -y 24 | Out-Null
Start-Sleep -Milliseconds 1800
P split-window -t "${s}:0.0" -c $X 'ping -n 60 127.0.0.1' | Out-Null
Start-Sleep -Milliseconds 3000

$cp = Get-PanePath "${s}:0.1"
if ($cp.TrimEnd('\') -ieq $X.TrimEnd('\')) {
    Write-Pass "split-window -c X <command> still runs the command in X"
} else {
    Write-Fail "split-window -c X <command> ran in '$cp', expected '$X'"
}

# Kill just that pane; its ConPTY host dies with it. If the host had also been
# parked in X, this would still have been enough, so the real signal is arm 1.
# Here we confirm the command path leaves nothing behind either.
P kill-pane -t "${s}:0.1" | Out-Null
Start-Sleep -Milliseconds 1500
$r = Test-Delete $X
if ($r.ok) {
    Write-Pass "X is released once the command pane is gone"
} else {
    Write-Fail "X still pinned after the command pane was killed. $($r.err)"
}
Kill-Sess $s

# ---- cleanup ----------------------------------------------------------------
foreach ($n in @('i630awarm', 'i630acold', 'i630bwarm', 'i630bcold', 'i630cwarm', 'i630ccold', 'i630dwarm', 'i630dcold', 'i630e')) {
    P kill-session -t $n 2>&1 | Out-Null
}
P kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 1500
Get-Process psmux -EA SilentlyContinue |
    Where-Object { $_.Path -eq $PSMUX } |
    ForEach-Object { Write-Info "cleaning up server pid $($_.Id)"; Stop-Process -Id $_.Id -Force -EA SilentlyContinue }
Start-Sleep -Milliseconds 800
Remove-Item -LiteralPath $ROOT -Recurse -Force -EA SilentlyContinue

$env:PSMUX_DATA_DIR = $savedData
$env:PSMUX_NO_WARM = $savedWarm
$env:PSMUX_SESSION_NAME = $savedSess
$env:PSMUX_TARGET_SESSION = $savedTarget

Write-Host ""
Write-Host "Passed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
