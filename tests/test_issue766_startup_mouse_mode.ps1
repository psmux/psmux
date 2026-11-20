# Issue #766 follow ups, from the reporter's last comment:
#
#  A. "a mouse mode requested at pane start is lost": a full screen app writes
#     ESC[?1049h ESC[?2004h ESC[?1000h ESC[?1003h ESC[?1006h as its very first
#     output, then enters raw mode the way node does (libuv assigns
#     ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT), and a few seconds
#     later the front end sends SGR wheel-ups.  3.3.8 delivered them, master
#     delivers nothing and #{mouse_any_flag}#{mouse_sgr_flag} reads 00.
#
#     Measured cause: the pane's console host ABSORBS a mouse DECSET written
#     while the console does not have ENABLE_VIRTUAL_TERMINAL_INPUT, and
#     forwards it only once VT input is on.  ?1049h and ?2004h go through, the
#     three mouse modes never leave conhost, so psmux (or any terminal hosting
#     the app straight through ConPTY: section 1 runs it with no psmux at all)
#     is never told the app wants the mouse.  3.3.8 forwarded the wheel on the
#     alternate screen alone and delivered it by accident; the #598 gate
#     (1357bc1b, tmux input_key_mouse parity) writes a report only to a pane
#     whose app asked for the mouse, and the request is invisible here.
#
#     Checked: the same request written after raw mode is forwarded, and the
#     wheel reaches the app (section 2); PSMUX_FORCE_WHEEL / @mouse-force
#     deliver it for an app that requested it too early (section 3).  The
#     startup only case itself is REPORTED, not judged, because the outcome is
#     decided by the console host.
#
#  B. `ESC ESC` written to the client in one write reached the pane as one
#     ESC on the console input route.  conhost hands it over as ONE Alt+Escape
#     key record and the client dropped the Alt.  tmux reads ESC ESC as
#     M-Escape and writes it to the pane as \033\033 (input-keys.c).  Both
#     input routes must deliver ESC ESC, then a bare ESC, then `a`, intact.
#
# Uses only csc: tests\mouse766_child.cs (the pane program, logs raw stdin) and
# tests\conpty_script766.cs (runs the psmux client under a real pseudoconsole
# and plays the front end's writes).  Every case runs in its own -L namespace
# and cleans up only that namespace.  PSMUX_TEST_BIN selects the binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$Reps = if ($env:PSMUX_TEST_REPS) { [int]$env:PSMUX_TEST_REPS } else { 3 }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

Write-Host "=== Issue #766: startup mouse mode request, ESC ESC ===" -ForegroundColor Cyan
Write-Info "binary: $PSMUX"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
if (-not (Test-Path $csc)) { Write-Skip "csc.exe not found"; exit 0 }

$root = Join-Path $env:TEMP ("psmux_i766m_" + (Get-Random -Maximum 999999))
New-Item -ItemType Directory -Force $root | Out-Null
$hostExe = Join-Path $root "conpty_script766.exe"
$childExe = Join-Path $root "mouse766_child.exe"
& $csc /nologo /platform:x64 /out:$hostExe "$PSScriptRoot\conpty_script766.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$childExe "$PSScriptRoot\mouse766_child.cs" 2>&1 | Out-Null
if (-not (Test-Path $hostExe) -or -not (Test-Path $childExe)) {
    Write-Fail "could not build the C# harnesses (csc at $csc)"
    exit 1
}

$scrub = "TMUX","TMUX_PANE","PSMUX_SESSION","PSMUX_VT_INPUT","PSMUX_FORCE_WHEEL","TERM_PROGRAM","WEZTERM_PANE","TERMINAL_EMULATOR","SSH_CONNECTION","SSH_CLIENT","SSH_TTY"
$saved = @{}
foreach ($v in $scrub) {
    $saved[$v] = [Environment]::GetEnvironmentVariable($v)
    Remove-Item "Env:\$v" -EA SilentlyContinue
}
$conf = Join-Path $root "i766.conf"
"set -g mouse on`nset -g escape-time 0`nset -g status off" | Set-Content -Encoding ascii $conf
$WHEEL = "1b 5b 3c 36 34 3b 31 30 3b 31 30 4d"
$defaultBefore = (& $PSMUX ls 2>&1 | Out-String).Trim()

function Get-Stdin([string]$Log) {
    if (-not (Test-Path $Log)) { return "" }
    $hex = (Get-Content $Log | Where-Object { $_ -match "^\d+ stdin \d+B " } | ForEach-Object { ($_ -split " ", 4)[3] }) -join " "
    return $hex.Trim()
}

# One psmux case: the pane program is the session's initial command.
# Returns flags sampled before the wheels and the pane's stdin.
function Invoke-PsmuxCase([string]$Tag, [string]$ChildArgs, [string]$Script, [hashtable]$Env = @{}) {
    $ns = "i766m_" + $Tag + "_" + (Get-Random -Maximum 999999)
    $log = Join-Path $root "$ns.log"
    $scr = Join-Path $root "$ns.scr"
    $Script | Set-Content -Encoding ascii $scr
    foreach ($k in $Env.Keys) { Set-Item "Env:\$k" $Env[$k] }
    $cmd = "`"$PSMUX`" -L $ns -f `"$conf`" new-session -A -s p -- `"$childExe`" `"$log`" $ChildArgs"
    $p = Start-Process -FilePath $hostExe -ArgumentList @("`"$log`"", "`"$scr`"", $cmd) -PassThru -WindowStyle Hidden
    $t0 = Get-Date
    while (((Get-Date) - $t0).TotalSeconds -lt 20) {
        if ((Test-Path $log) -and ((Get-Content $log -Raw) -match " ready")) { break }
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Milliseconds 2600
    $flags = (& $PSMUX -L $ns display-message -p -t p '#{mouse_any_flag}#{mouse_sgr_flag}' 2>&1 | Out-String).Trim()
    if (-not $p.WaitForExit(40000)) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    foreach ($k in $Env.Keys) { Remove-Item "Env:\$k" -EA SilentlyContinue }
    $stdin = Get-Stdin $log
    $ready = (Test-Path $log) -and ((Get-Content $log -Raw) -match " ready")
    return [pscustomobject]@{ Ready = $ready; Flags = $flags; Stdin = $stdin
        Wheels = ([regex]::Matches($stdin, [regex]::Escape("1b 5b 3c 36 34 3b"))).Count }
}

$wheelScript = "ready ready`nsleep 3000`nwrite $WHEEL`nsleep 150`nwrite $WHEEL`nsleep 150`nwrite $WHEEL`nsleep 1500"

try {
    # ── 1. no psmux: what a terminal hosting the app through ConPTY is told ──
    Write-Host "`n[1] the app straight under a pseudoconsole (no psmux): which modes reach the terminal" -ForegroundColor Cyan
    foreach ($case in @(@("startup", "libuv"), @("repeated after raw mode", "libuv again"))) {
        $log = Join-Path $root ("direct_" + (Get-Random) + ".log")
        $out = "$log.out"
        $scr = "$log.scr"
        "ready ready`nsleep 3000" | Set-Content -Encoding ascii $scr
        $env:CONPTY766_OUT = $out
        $p = Start-Process -FilePath $hostExe -ArgumentList @("`"$log`"", "`"$scr`"", "`"$childExe`" `"$log`" $($case[1])") -PassThru -WindowStyle Hidden
        if (-not $p.WaitForExit(30000)) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
        Remove-Item Env:\CONPTY766_OUT -EA SilentlyContinue
        $txt = if (Test-Path $out) { [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($out)) } else { "" }
        $modes = ([regex]::Matches($txt, "\x1b\[\?(1000|1002|1003|1006)[;0-9]*h") | ForEach-Object { $_.Value.Substring(1) }) -join " "
        if (-not $modes) { $modes = "none" }
        Write-Info "[$($case[0])] mouse modes the terminal was told: $modes"
    }

    # ── 2. the request written after raw mode reaches psmux ─────────────────
    Write-Host "`n[2] the request repeated after raw mode: flags and the wheel" -ForegroundColor Cyan
    # The wheel-ups are written as SGR reports into the client's own pseudo
    # console, and the inbox one of Windows 10 (19045) has no ConPTY mouse
    # (#457): it hands the client no wheel at all, so the wheel half cannot be
    # driven there. The same checks pass on Windows 11 with the build psmux
    # sees faked to 19045 (PSMUX_FAKE_WIN_BUILD).
    $NoMouseIn = [Environment]::OSVersion.Version.Build -lt 22000
    $ok = 0; $ready = 0; $flagsOk = 0
    for ($i = 1; $i -le $Reps; $i++) {
        $r = Invoke-PsmuxCase "again$i" "libuv again" $wheelScript
        if (-not $r.Ready) { continue }
        $ready++
        if ($r.Wheels -eq 3) { $ok++ }
        if ($r.Flags -eq "11") { $flagsOk++ }
    }
    if ($ready -eq 0) { Write-Fail "the pane never came up" }
    else {
        if ($flagsOk -eq $ready) { Write-Pass "mouse_any_flag and mouse_sgr_flag read 11 in $flagsOk/$ready runs" }
        else { Write-Fail "flags read 11 in only $flagsOk/$ready runs" }
        if ($ok -eq $ready) { Write-Pass "all 3 wheel-ups reached the app in $ok/$ready runs" }
        elseif ($NoMouseIn) { Write-Skip "Windows 10 pseudo console hands the client no wheel ($ok/$ready runs)" }
        else { Write-Fail "all 3 wheel-ups reached the app in only $ok/$ready runs" }
    }

    # ── 3. the request written only at startup ──────────────────────────────
    Write-Host "`n[3] the request written only at startup (before raw mode)" -ForegroundColor Cyan
    $got = 0; $ready = 0; $fl = @()
    for ($i = 1; $i -le $Reps; $i++) {
        $r = Invoke-PsmuxCase "start$i" "libuv" $wheelScript
        if (-not $r.Ready) { continue }
        $ready++; $got += $r.Wheels; $fl += $r.Flags
    }
    Write-Info "default: $got of $(3 * $ready) wheel-ups reached the app, flags $($fl -join ',') (the console host withheld the request; see the header)"
    $ok = 0; $ready = 0
    for ($i = 1; $i -le $Reps; $i++) {
        $r = Invoke-PsmuxCase "force$i" "libuv" $wheelScript @{ PSMUX_FORCE_WHEEL = "1" }
        if (-not $r.Ready) { continue }
        $ready++
        if ($r.Wheels -eq 3) { $ok++ }
    }
    if ($ready -eq 0) { Write-Fail "[force] the pane never came up" }
    elseif ($ok -eq $ready) { Write-Pass "PSMUX_FORCE_WHEEL=1 delivers all 3 wheel-ups to the early requester in $ok/$ready runs" }
    elseif ($NoMouseIn) { Write-Skip "[force] Windows 10 pseudo console hands the client no wheel ($ok/$ready runs)" }
    else { Write-Fail "PSMUX_FORCE_WHEEL=1 delivered all 3 wheel-ups in only $ok/$ready runs" }

    # ── 4. ESC ESC ──────────────────────────────────────────────────────────
    Write-Host "`n[4] ESC ESC in one write, then a bare ESC, then a" -ForegroundColor Cyan
    $escScript = "ready ready`nsleep 1500`nwrite 1b 1b`nsleep 800`nwrite 1b`nsleep 800`nwrite 61`nsleep 800"
    foreach ($route in "console", "vt") {
        $ok = 0; $ready = 0; $seen = @()
        $envs = if ($route -eq "vt") { @{ PSMUX_VT_INPUT = "1" } } else { @{} }
        for ($i = 1; $i -le $Reps; $i++) {
            $r = Invoke-PsmuxCase "esc$route$i" "plain" $escScript $envs
            if (-not $r.Ready) { continue }
            $ready++
            if ($r.Stdin -eq "1b 1b 1b 61") { $ok++ } else { $seen += "[$($r.Stdin)]" }
        }
        if ($ready -eq 0) { Write-Fail "[$route] the pane never came up"; continue }
        if ($ok -eq $ready) { Write-Pass "[$route route] the pane read 1b 1b, 1b, 61 in $ok/$ready runs" }
        else { Write-Fail "[$route route] the pane read 1b 1b, 1b, 61 in only $ok/$ready runs, otherwise $($seen -join ' ')" }
    }
}
finally {
    foreach ($v in $scrub) {
        if ($null -ne $saved[$v]) { Set-Item "Env:\$v" $saved[$v] } else { Remove-Item "Env:\$v" -EA SilentlyContinue }
    }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

$defaultAfter = (& $PSMUX ls 2>&1 | Out-String).Trim()
if ($defaultAfter -ne $defaultBefore) { Write-Info "the default namespace's session list changed during the run (another user of it?)" }

Write-Host ""
Write-Host "=== Results: $($script:Pass) passed, $($script:Fail) failed, $($script:Skip) skipped ===" -ForegroundColor $(if ($script:Fail -eq 0) { "Green" } else { "Red" })
exit $(if ($script:Fail -eq 0) { 0 } else { 1 })
