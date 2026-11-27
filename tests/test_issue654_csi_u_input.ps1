# Issue #654: "CSI u key sequences are mishandled"
#
# `CSI <code> ; <modifiers> u` (modifyOtherKeys / fixterms) is how a terminal
# reports a key legacy VT cannot encode: CR is CR whether or not Shift is down,
# and C0 collapses whole families of Ctrl combinations onto one byte.  psmux
# parsed it nowhere, and because it has two input paths the one gap surfaced as
# two failures in opposite directions:
#
#   console records (Windows Terminal, conhost)  the sequence reached the pane
#                                                as the TEXT `[13;2u`
#   VT bytes (SSH, WezTerm, JetBrains)           the key was discarded outright
#
# Measured on master before the fix, with the parts below:
#
#   console path  the pane's child received `[13;2u`            3/3 runs
#   VT path       the pane's child received nothing at all      3/3 runs
#
# WHY THE CONSOLE PATH LOOKS LIKE THAT.  conhost's input parser does not know
# `CSI u`, so it flushes the unrecognised sequence into the console input buffer
# one KEY_EVENT per byte, every record carrying vk 0, scan 0 and the byte in
# UnicodeChar, all of them in ONE ReadConsoleInputW.  The ESC record does not
# survive crossterm (no virtual key code, a control UnicodeChar, ToUnicodeEx
# answers nothing for a synthesised record), so psmux is handed six bare
# characters and its paste heuristic turns them into text.  tests/csiinject654.cs
# reproduces exactly that shape, byte for byte, in one WriteConsoleInput call.
#
# HARNESS.  Two, one per path.  The console parts host a real attached client in
# its own console window and write records into it.  The VT part puts the client
# under test inside an OUTER psmux pane with the WezTerm environment set, which
# is what `needs_vt_input()` routes onto the byte parser, and `send-keys -H` on
# the outer pane puts the raw bytes on its stdin exactly as a terminal would.
#
# WHAT THE PANE'S CHILD IS.  `stty raw -echo; cat -v` under Git Bash, so every
# byte psmux writes to the pane is visible as itself: ESC prints as `^[`, CR as
# `^M`, a tab as a tab.  A test that reads a shell prompt cannot tell a decoded
# key from text that happens to look like one; this one can.
#
# Layers: E2E over real consoles, native INPUT_RECORD injection, VT input path
#         over a real ConPTY, key bindings, paste regression guard.
#
# Windows 10 (a build before 22000): in Part D the outer pane's inbox conhost
# 19045 hands the client inside no CSI 13;2u at all (the inner pane's child
# got nothing in 3 of 3 runs, while the control's plain bytes arrive), as it
# passes a VT reader no key sequence it does not know. So there Part D expects
# nothing to arrive. With PSMUX_CONPTY_DIR the panes run on OpenConsole, which
# hands it on as Windows 11 does (3 of 3 runs on 19045), so Part D expects it
# there, as test_altgr_letter_paste.ps1 does.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE }
         elseif ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE }
         else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }

$tmp = Join-Path $env:TEMP "psmux_issue654"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

$script:TestsPassed  = 0
$script:TestsFailed  = 0
$script:TestsSkipped = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

# Every window and process this suite opens is tracked here and closed by pid.
$script:OpenedPids = @()

Write-Host "`n=== Issue #654: CSI u extended keys on input ===" -ForegroundColor Cyan
Write-Info "psmux under test: $PSMUX"

# ── The pane's child: a byte logger, so a key can be told from text ──────────
$bash = $null
foreach ($cand in @("C:\Program Files\Git\bin\bash.exe",
                    "C:\Program Files (x86)\Git\bin\bash.exe",
                    "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe")) {
    if (Test-Path $cand) { $bash = $cand; break }
}
if (-not $bash) {
    $bcmd = Get-Command bash -EA SilentlyContinue
    if ($bcmd) { $bash = $bcmd.Source }
}
$logger = Join-Path $tmp "bytelog654.cmd"
if ($bash) {
    "@echo off`r`n`"$bash`" -c `"stty raw -echo; cat -v`"`r`n" | Set-Content $logger -Encoding ASCII
}

# ── The record injector ─────────────────────────────────────────────────────
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$injSrc = Join-Path $PSScriptRoot "csiinject654.cs"
$injExe = Join-Path $tmp "csiinject654.exe"
if ((Test-Path $csc) -and (Test-Path $injSrc)) {
    & $csc /nologo /optimize /out:$injExe $injSrc 2>&1 | Out-Null
}

function Write-Conf($name, $lines) {
    $path = Join-Path $tmp "$name.conf"
    (($lines -join "`n") + "`n") | Set-Content $path -Encoding UTF8
    return $path
}

# Bring up an attached client in its own console window, with the byte logger as
# the pane's child, and hand back the client's pid so records can be written
# into its console input buffer.
function Start-Console($ns, $sess, $conf) {
    # Two attempts: bringing a fresh server and an attached client up is the
    # slow part of this harness and loses the occasional race.  A run that
    # never started is not evidence about a key, so it is never scored.
    foreach ($attempt in 1..2) {
        & $PSMUX -L $ns kill-session -t $sess 2>&1 | Out-Null
        Start-Sleep -Milliseconds 400
        $env:PSMUX_NO_WARM = "1"
        $a = @("-L", $ns)
        if ($conf) { $a += @("-f", $conf) }
        $a += @("new-session", "-s", $sess, "-x", "100", "-y", "30", $logger)
        $proc = Start-Process -FilePath $PSMUX -ArgumentList $a -PassThru
        Remove-Item Env:PSMUX_NO_WARM -EA SilentlyContinue
        $script:OpenedPids += $proc.Id
        # Poll rather than guess: a cold server plus an attached client takes a
        # few seconds, and longer when several of them have just been torn down.
        $ok = $false
        foreach ($tick in 1..20) {
            Start-Sleep -Seconds 1
            & $PSMUX -L $ns has-session -t $sess 2>$null
            if ($LASTEXITCODE -eq 0) { $ok = $true; break }
        }
        if ($ok) {
            Start-Sleep -Seconds 2   # let the pane's child finish coming up
            return $proc
        }
        try { if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } } catch {}
        $script:OpenedPids = $script:OpenedPids | Where-Object { $_ -ne $proc.Id }
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Seconds 1
    }
    return $null
}

function Stop-Console($ns, $sess, $proc) {
    & $PSMUX -L $ns kill-session -t $sess 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    if ($proc) {
        try { if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } } catch {}
        $script:OpenedPids = $script:OpenedPids | Where-Object { $_ -ne $proc.Id }
    }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$psmuxDir\${ns}__$sess.*" -Force -EA SilentlyContinue
}

function Capture($ns, $sess) {
    return ((& $PSMUX -L $ns capture-pane -t $sess -p 2>&1 | Out-String).TrimEnd())
}

# A capture that came back as a CLI refusal is not evidence about a key: the
# session went away underneath the run.  Such a run is not scored either way.
function Is-LiveCapture($cap) {
    return -not ($cap -match "no server running|no such session|can't find")
}

function PaneCount($ns, $sess) {
    return [int]((& $PSMUX -L $ns display-message -t $sess -p '#{window_panes}' 2>&1 | Out-String).Trim())
}

$ready = $true
if (-not $bash)               { Write-Skip "Git Bash is missing, so no byte logger can run"; $ready = $false }
if (-not (Test-Path $injExe)) { Write-Skip "the record injector did not compile"; $ready = $false }

# ==========================================================================
# PART A: the console path, with the reported seven records
# ==========================================================================
Write-Host "`n[Part A] Console records: CSI 13;2u must arrive as Shift+Enter" -ForegroundColor Yellow

if ($ready) {
    $runs = 3; $hit = 0; $text = 0; $done = 0
    for ($i = 1; $i -le $runs; $i++) {
        $ns = "i654a$i"; $s = "a654r$i"
        $proc = Start-Console $ns $s $null
        if (-not $proc) { Write-Info "run ${i}: the client did not come up"; continue }
        # The seven records conhost delivers for one press of Shift+Enter, in
        # ONE WriteConsoleInput call: ESC [ 1 3 ; 2 u, vk 0, scan 0, key down.
        & $injExe $proc.Id "hexseq:1b,5b,31,33,3b,32,75" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $cap = Capture $ns $s
        Write-Info "run ${i}: the child received [$($cap -replace "`r?`n", '\n')]"
        if (Is-LiveCapture $cap) {
            $done++
            if ($cap -match "\^\[\^M") { $hit++ }
            if ($cap -match "\[13;2u") { $text++ }
        }
        Stop-Console $ns $s $proc
    }
    if ($done -eq 0) {
        Write-Skip "no attached client came up, so the console path went untested"
    } elseif ($hit -eq $done) {
        Write-Pass "$hit/$done runs: the pane's child received ESC CR, psmux's Shift+Enter encoding"
    } else {
        Write-Fail "only $hit/$done runs delivered ESC CR to the child"
    }
    if ($done -gt 0 -and $text -eq 0) {
        Write-Pass "0/$done runs typed the literal text [13;2u into the pane  <-- the #654 console half"
    } elseif ($done -gt 0) {
        Write-Fail "$text/$done runs still typed the sequence into the pane as text"
    }
} else {
    Write-Skip "Part A needs Git Bash and the record injector"
}

# ==========================================================================
# PART B: the decoded keys are bindable
# ==========================================================================
Write-Host "`n[Part B] A decoded extended key reaches bind-key" -ForegroundColor Yellow

if ($ready) {
    foreach ($case in @(
        @{ name = "S-Enter"; conf = @("bind -n S-Enter split-window -v"); spec = "hexseq:1b,5b,31,33,3b,32,75"; seq = "CSI 13;2u" },
        @{ name = "C-Tab";   conf = @("bind -n C-Tab split-window -v");   spec = "hexseq:1b,5b,39,3b,35,75";    seq = "CSI 9;5u" },
        @{ name = "C-i";     conf = @("bind -n C-i split-window -v");     spec = "hexseq:1b,5b,31,30,35,3b,35,75"; seq = "CSI 105;5u" }
    )) {
        $ns = "i654b" + $case.name.Replace("-", "")
        $s  = "b654" + $case.name.Replace("-", "")
        $conf = Write-Conf ("bind" + $case.name.Replace("-", "")) $case.conf
        $proc = Start-Console $ns $s $conf
        if (-not $proc) { Write-Fail "$($case.name): the client did not come up"; continue }
        $before = PaneCount $ns $s
        & $injExe $proc.Id $case.spec 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1800
        $after = PaneCount $ns $s
        if ($after -gt $before) {
            Write-Pass "$($case.seq) fired the bind -n $($case.name) binding (panes $before -> $after)"
        } else {
            Write-Fail "$($case.seq) did not fire the bind -n $($case.name) binding (panes $before -> $after)"
        }
        Stop-Console $ns $s $proc
    }
} else {
    Write-Skip "Part B needs Git Bash and the record injector"
}

# ==========================================================================
# PART C: a paste that CONTAINS the sequence is still text
# ==========================================================================
Write-Host "`n[Part C] Paste regression guard" -ForegroundColor Yellow

# A pasted clipboard reaches a console as one burst of character records, the
# same shape an unparsed sequence arrives in.  Text that happens to contain
# `[13;2u` must not be decoded into a key: on the first cut of the fix a paste
# of `PA[13;2uXY` reached the child as `PA` CR `XY`, six characters of the
# user's own text gone.  The `[` of a real sequence opens its burst, because
# the console dropped the ESC in front of it; inside a paste it does not.
if ($ready) {
    $runs = 3; $verbatim = 0; $done = 0
    for ($i = 1; $i -le $runs; $i++) {
        $ns = "i654c$i"; $s = "c654r$i"
        $proc = Start-Console $ns $s $null
        if (-not $proc) { Write-Info "run ${i}: the client did not come up"; continue }
        & $injExe $proc.Id "burst:PA[13;2uXY" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1800
        $cap = Capture $ns $s
        Write-Info "run ${i}: the child received [$($cap -replace "`r?`n", '\n')]"
        if (Is-LiveCapture $cap) {
            $done++
            if ($cap -match "PA\[13;2uXY") { $verbatim++ }
        }
        Stop-Console $ns $s $proc
    }
    if ($done -eq 0) {
        Write-Skip "no attached client came up, so the paste guard went untested"
    } elseif ($verbatim -eq $done) {
        Write-Pass "$verbatim/$done runs: a paste carrying [13;2u reached the child verbatim"
    } else {
        Write-Fail "only $verbatim/$done runs kept the pasted text whole"
    }

    # And a `[` somebody typed is still a `[`, with nothing held back.
    $ns = "i654ct"; $s = "c654typed"
    $proc = Start-Console $ns $s $null
    if (-not $proc) {
        Write-Fail "typed bracket: the client did not come up"
    } else {
        & $injExe $proc.Id "type:[ab:60" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $cap = Capture $ns $s
        Write-Info "typed: the child received [$($cap -replace "`r?`n", '\n')]"
        if ($cap -match "\[ab") {
            Write-Pass "a typed bracket still goes straight through, with the characters behind it"
        } else {
            Write-Fail "a typed bracket no longer reaches the pane intact (got [$cap])"
        }
        Stop-Console $ns $s $proc
    }
} else {
    Write-Skip "Part C needs Git Bash and the record injector"
}

# ==========================================================================
# PART D: the VT path, which is SSH, WezTerm and the JetBrains terminals
# ==========================================================================
Write-Host "`n[Part D] VT bytes: CSI 13;2u over a real ConPTY" -ForegroundColor Yellow

$NS_OUT = "i654out"; $NS_IN = "i654in"

function Start-Link($idx, $sessOut, $sessIn) {
    & $PSMUX -L $NS_IN  kill-session -t $sessIn  2>&1 | Out-Null
    & $PSMUX -L $NS_OUT kill-session -t $sessOut 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    & $PSMUX -L $NS_OUT new-session -d -s $sessOut -x 120 -y 40 2>&1 | Out-Null
    $outUp = $false
    foreach ($tick in 1..15) {
        Start-Sleep -Seconds 1
        & $PSMUX -L $NS_OUT has-session -t $sessOut 2>$null
        if ($LASTEXITCODE -eq 0) { $outUp = $true; break }
    }
    if (-not $outUp) { return $false }
    Start-Sleep -Seconds 2
    # PSMUX_SESSION must go or the inner client refuses to nest.  TERM_PROGRAM
    # and WEZTERM_PANE are what put it on the VT input path, exactly as a real
    # WezTerm window would (ssh_input::needs_vt_input).
    $cmd = "Remove-Item Env:\PSMUX_SESSION,Env:\PSMUX_SESSION_NAME,Env:\PSMUX_PANE -EA SilentlyContinue; " +
           "`$env:TERM_PROGRAM='WezTerm'; `$env:WEZTERM_PANE='0'; `$env:PSMUX_NO_WARM='1'; " +
           "& '$PSMUX' -L $NS_IN new-session -s $sessIn '$logger'"
    & $PSMUX -L $NS_OUT send-keys -t $sessOut $cmd Enter 2>&1 | Out-Null
    foreach ($tick in 1..20) {
        Start-Sleep -Seconds 1
        & $PSMUX -L $NS_IN has-session -t $sessIn 2>$null
        if ($LASTEXITCODE -eq 0) { Start-Sleep -Seconds 2; return $true }
    }
    return $false
}

function Stop-Link($sessOut, $sessIn) {
    & $PSMUX -L $NS_IN  kill-session -t $sessIn  2>&1 | Out-Null
    & $PSMUX -L $NS_OUT kill-session -t $sessOut 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    # Both namespaces go with the link: the next run brings its own server up,
    # and a half torn down one races it into "no server running".
    & $PSMUX -L $NS_IN  kill-server 2>&1 | Out-Null
    & $PSMUX -L $NS_OUT kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 1
}

if (-not $bash) {
    Write-Skip "Part D needs Git Bash for the byte logger"
} else {
    $runs = 3; $hit = 0; $done = 0
    for ($i = 1; $i -le $runs; $i++) {
        $so = "o654r$i"; $si = "i654r$i"
        $up = Start-Link $i $so $si
        if (-not $up) {
            # Bringing two nested servers up is the slow part of this harness,
            # and it loses the occasional race; one retry, then give up on the
            # run rather than score the product for it.
            Stop-Link $so $si
            $up = Start-Link $i $so $si
        }
        if (-not $up) {
            Write-Info "run ${i}: the ConPTY link did not come up"
            Stop-Link $so $si
            continue
        }
        # The bytes a terminal puts on the client's stdin for Shift+Enter.
        & $PSMUX -L $NS_OUT send-keys -t $so -H 1b 5b 31 33 3b 32 75 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $cap = ((& $PSMUX -L $NS_IN capture-pane -t $si -p 2>&1 | Out-String).TrimEnd())
        Write-Info "run ${i}: the inner pane's child received [$($cap -replace "`r?`n", '\n')]"
        $done++
        if ($cap -match "\^\[\^M") { $hit++ }
        Stop-Link $so $si
    }
    if ($done -eq 0) {
        Write-Skip "no ConPTY link came up, so the VT path went untested"
    } elseif ([Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR) {
        if ($hit -eq 0) { Write-Pass "0/$done runs: the outer pane's inbox conhost hands the client inside no CSI 13;2u on this host" }
        else { Write-Fail "$hit/$done runs got CSI 13;2u through the inbox conhost of Windows 10, which passed none when measured" }
    } elseif ($hit -eq $done) {
        Write-Pass "$hit/$done runs: the VT path decoded CSI 13;2u  <-- the #654 silent half"
    } else {
        Write-Fail "only $hit/$done runs decoded CSI 13;2u on the VT path"
    }

    # The harness itself has to be able to deliver a byte, or "nothing arrived"
    # would prove nothing at all.
    $so = "o654ctl"; $si = "i654ctl"
    $up = Start-Link 0 $so $si
    if (-not $up) {
        Stop-Link $so $si
        $up = Start-Link 0 $so $si
    }
    if (-not $up) {
        Write-Skip "control: the ConPTY link did not come up"
    } else {
        & $PSMUX -L $NS_OUT send-keys -t $so -H 41 0d 42 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $cap = ((& $PSMUX -L $NS_IN capture-pane -t $si -p 2>&1 | Out-String).TrimEnd())
        if ($cap -match "A\^MB") {
            Write-Pass "control: ordinary bytes still reach the inner pane's child (got [$cap])"
        } else {
            Write-Fail "control: the harness could not deliver plain bytes (got [$cap])"
        }
    }
    Stop-Link $so $si
}

# ==========================================================================
# Close everything this suite opened, by pid, and drop its state files.
# ==========================================================================
foreach ($openPid in $script:OpenedPids) {
    try { Stop-Process -Id $openPid -Force -EA SilentlyContinue } catch {}
}
foreach ($ns in @($NS_IN, $NS_OUT, "i654ct", "i654bSEnter", "i654bCTab", "i654bCi")) {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
}
for ($i = 1; $i -le 3; $i++) {
    & $PSMUX -L "i654a$i" kill-server 2>&1 | Out-Null
    & $PSMUX -L "i654c$i" kill-server 2>&1 | Out-Null
}
Start-Sleep -Milliseconds 800
Remove-Item "$psmuxDir\i654*" -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\o654*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor DarkYellow
exit $script:TestsFailed
