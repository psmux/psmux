# Issue #615: "pane_current_path seems to be not updated correctly in wsl" (fekir)
#
# Reported:
#     bind '"' split-window -c "#{pane_current_path}"
#     cd C:/ ; psmux ; wsl ; cd Users ; Ctrl+B "
#         -> the new pane opens in C:\ instead of C:\Users
#   and the same recipe through cygwin bash tracks the path correctly.
#
# MEASURED ROOT CAUSE (tests/proc_cwd_probe.cs reads the PEB
# RTL_USER_PROCESS_PARAMETERS.CurrentDirectory of every process in the pane's
# tree, i.e. exactly what GetCurrentDirectory would return inside it):
#
#   pwsh        pane 53196 pwsh     CWD=C:\      -> after `cd C:\Users`  CWD=C:\Users\
#   cygwin bash pane 27080 bash     CWD=C:\cygwin64\home\godwin
#                                                -> after `cd /cygdrive/c/Users` CWD=C:\Users\
#   git bash    pane 25524 bash     CWD=C:\      -> after `cd /c/Users`   CWD=C:\Users\
#   wsl         pane 28936 pwsh     CWD=C:\
#                    12740 wsl      CWD=C:\
#                    18820 wsl      CWD=C:\
#                    44204 wslhost  CWD=C:\      -> after `cd /mnt/c/Users` ALL STILL C:\
#
# cygwin and msys2 `cd` call SetCurrentDirectory, so the Win32 cwd of the shell
# process follows the Unix cwd and psmux reads the right answer.  Under `wsl`
# the shell is a Linux process inside the VM; every Win32 process in the pane
# (wsl.exe, wslhost.exe) keeps the cwd it was created with forever, so there is
# no Win32 cwd to read.  This is the same situation tmux is in when
# osdep_get_cwd() fails (format.c:965 format_cb_current_path) -- it returns
# NULL and the caller falls back to the last known path.
#
# The only way for a Linux shell to tell the multiplexer where it is is shell
# integration: OSC 7 (`ESC ] 7 ; file://host/path ESC \`) or ConEmu's OSC 9;9.
# psmux already parsed OSC 7 into #{pane_path}, but stored it RAW
# (`file:///C:/Windows/Temp` came back as the literal `/C:/Windows/Temp`),
# never handled OSC 9;9 at all, and #{pane_current_path} never consulted it.
#
# The fix: translate an OSC 7/9;9 payload into a native Windows path
# (/mnt/c/Users -> C:\Users, /home/gj -> \\wsl.localhost\Ubuntu\home\gj,
# file:///C:/x -> C:\x, percent decoding), accept OSC 9;9, and let
# #{pane_current_path} take whichever of the OSC report and the Win32 cwd
# changed most recently.  Panes that never emit OSC keep the Win32 cwd exactly
# as before.
#
# Layers: real detached sessions driven over the CLI, real pwsh / wsl / cygwin
# bash / git bash children, the PEB probe as the ground truth for the Win32
# cwd, and a real `split-window -c "#{pane_current_path}"` whose new pane is
# asked where it actually landed.
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$SOCK = "i615"
$TMP = Join-Path $env:TEMP "psmux_615"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null

$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Sect($m) { Write-Host "`n=== $m ===" -ForegroundColor Cyan }

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "dataDir: $(if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" })" -ForegroundColor Cyan

$emptyConf = Join-Path $TMP "empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

$script:Sessions = @()

function New-Sess([string]$n, [string]$cwd) {
    & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    & $PSMUX -L $SOCK -f $emptyConf new-session -d -s $n -c $cwd -x 120 -y 30 2>&1 | Out-Null
    if ($script:Sessions -notcontains $n) { $script:Sessions += $n }
    Start-Sleep -Seconds 3
}
function Kill-Sess([string]$n) { & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null }
function Cleanup { foreach ($s in $script:Sessions) { Kill-Sess $s }; Start-Sleep -Milliseconds 500 }

function CurPath([string]$t) { (& $PSMUX -L $SOCK display-message -p -t $t '#{pane_current_path}' 2>&1 | Out-String).Trim() }
function PanePath([string]$t) { (& $PSMUX -L $SOCK display-message -p -t $t '#{pane_path}' 2>&1 | Out-String).Trim() }
function Cap([string]$t) { (& $PSMUX -L $SOCK capture-pane -p -t $t 2>&1 | Out-String) }
function SK([string]$t, [string]$k) { & $PSMUX -L $SOCK send-keys -t $t $k Enter 2>&1 | Out-Null }

# Poll #{pane_current_path} until it reaches $want, up to $sec seconds.
function Wait-Path([string]$t, [string]$want, [int]$sec = 6) {
    $end = (Get-Date).AddSeconds($sec)
    do {
        $p = CurPath $t
        if ($p -eq $want) { return $p }
        Start-Sleep -Milliseconds 300
    } while ((Get-Date) -lt $end)
    return (CurPath $t)
}
function Wait-Cap([string]$t, [string]$pat, [int]$sec) {
    $end = (Get-Date).AddSeconds($sec)
    do {
        if ((Cap $t) -match $pat) { return $true }
        Start-Sleep -Milliseconds 400
    } while ((Get-Date) -lt $end)
    return $false
}

# Ask a freshly split pane where it really is, independent of any psmux format.
# The marker is built by concatenation so that the ECHO of the typed command
# line cannot match the regex we are looking for -- only the real output can.
function Real-Cwd([string]$t) {
    SK $t '[Console]::Out.Write("CWD"+"IS<" + (Get-Location).Path + ">")'
    $end = (Get-Date).AddSeconds(8)
    do {
        $c = Cap $t
        if ($c -match 'CWDIS<([^>]*)>') {
            foreach ($m in [regex]::Matches($c, 'CWDIS<([^>]*)>')) { $last = $m.Groups[1].Value }
            if ($last) { return $last }
        }
        Start-Sleep -Milliseconds 400
    } while ((Get-Date) -lt $end)
    return "<no answer>"
}

# The reporter's binding, executed for real.
function Split-AndVerify([string]$sess, [string]$expect, [string]$label) {
    $sp = CurPath "${sess}:0.0"
    & $PSMUX -L $SOCK split-window -d -t "${sess}:0.0" -c $sp 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $np = CurPath "${sess}:0.1"
    $real = Real-Cwd "${sess}:0.1"
    Write-Info "$label split-window -c '$sp' -> new pane_current_path='$np' real cwd='$real'"
    $realOk = $real.TrimEnd('\') -eq $expect.TrimEnd('\')
    if ($np -eq $expect -and $realOk) {
        Write-Pass "$label split-window -c '#{pane_current_path}' lands in '$expect'"
    } else {
        Write-Fail "$label split-window landed at pane_current_path='$np' real='$real', expected '$expect'"
    }
    & $PSMUX -L $SOCK kill-pane -t "${sess}:0.1" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

# ---------------------------------------------------------------------------
# Availability probes
# ---------------------------------------------------------------------------
$cygBash = "C:\cygwin64\bin\bash.exe"
$hasCyg = Test-Path $cygBash
$gitBash = $null
foreach ($c in @("C:\Program Files\Git\bin\bash.exe", "C:\Program Files (x86)\Git\bin\bash.exe")) {
    if (Test-Path $c) { $gitBash = $c; break }
}
$wslDistro = $null
$wslHome = $null
try {
    $distros = @((& wsl.exe -l -q 2>$null) | ForEach-Object { ($_ -replace "`0", "").Trim() } | Where-Object { $_ })
    foreach ($d in $distros) {
        if ($d -match 'docker|podman') { continue }
        $probe = ((& wsl.exe -d $d -e sh -c 'echo OK:$HOME' 2>$null) -replace "`0", "").Trim()
        if ($probe -match '^OK:(.+)$') { $wslDistro = $d; $wslHome = $Matches[1]; break }
    }
} catch { }
Write-Info "cygwin bash : $(if ($hasCyg) { $cygBash } else { 'not installed' })"
Write-Info "git bash    : $(if ($gitBash) { $gitBash } else { 'not installed' })"
Write-Info "wsl distro  : $(if ($wslDistro) { "$wslDistro (HOME=$wslHome)" } else { 'not installed' })"

# ---------------------------------------------------------------------------
# Part A: pwsh control.  The Win32 cwd of the pane process follows `cd`, so
# this has always worked and must keep working.
# ---------------------------------------------------------------------------
Write-Sect "Part A: native pwsh pane (control, Win32 cwd follows cd)"
$SA = "i615_pwsh"
New-Sess $SA "C:\"
$p0 = CurPath "${SA}:0.0"
if ($p0 -eq "C:\") { Write-Pass "fresh pane created with -c C:\ reports '$p0'" }
else { Write-Fail "fresh pane reports '$p0', expected 'C:\'" }

SK "${SA}:0.0" 'cd C:\Users'
$p1 = Wait-Path "${SA}:0.0" "C:\Users"
if ($p1 -eq "C:\Users") { Write-Pass "pwsh `cd C:\Users` -> pane_current_path '$p1'" }
else { Write-Fail "pwsh `cd C:\Users` -> pane_current_path '$p1', expected 'C:\Users'" }
Split-AndVerify $SA "C:\Users" "pwsh"
Kill-Sess $SA

# ---------------------------------------------------------------------------
# Part B: cygwin bash control.  The reporter says this arm works; it does
# because cygwin's chdir() also calls SetCurrentDirectory.
# ---------------------------------------------------------------------------
Write-Sect "Part B: cygwin bash (control, reporter says this arm works)"
if (-not $hasCyg) {
    Write-Skip "cygwin not installed at $cygBash"
} else {
    $SB = "i615_cyg"
    New-Sess $SB "C:\"
    SK "${SB}:0.0" "& '$cygBash' --login -i"
    if (-not (Wait-Cap "${SB}:0.0" '\$' 45)) { Write-Info "cygwin prompt not seen, continuing anyway" }
    Start-Sleep -Seconds 2
    SK "${SB}:0.0" "cd /cygdrive/c/Users"
    $pb = Wait-Path "${SB}:0.0" "C:\Users" 8
    if ($pb -eq "C:\Users") { Write-Pass "cygwin `cd /cygdrive/c/Users` -> '$pb'" }
    else { Write-Fail "cygwin `cd /cygdrive/c/Users` -> '$pb', expected 'C:\Users'" }
    Split-AndVerify $SB "C:\Users" "cygwin"
    Kill-Sess $SB
}

# ---------------------------------------------------------------------------
# Part C: git bash (msys2) control.
# ---------------------------------------------------------------------------
Write-Sect "Part C: git bash / msys2 (control)"
if (-not $gitBash) {
    Write-Skip "git bash not installed"
} else {
    $SC = "i615_gitbash"
    New-Sess $SC "C:\"
    SK "${SC}:0.0" "& '$gitBash' --login -i"
    if (-not (Wait-Cap "${SC}:0.0" 'MINGW|\$' 45)) { Write-Info "git bash prompt not seen, continuing anyway" }
    Start-Sleep -Seconds 2
    SK "${SC}:0.0" "cd /c/Users"
    $pc = Wait-Path "${SC}:0.0" "C:\Users" 8
    if ($pc -eq "C:\Users") { Write-Pass "git bash `cd /c/Users` -> '$pc'" }
    else { Write-Fail "git bash `cd /c/Users` -> '$pc', expected 'C:\Users'" }
    Split-AndVerify $SC "C:\Users" "git bash"
    Kill-Sess $SC
}

# ---------------------------------------------------------------------------
# Part D: OSC acceptance and the no-regression rule, driven from a native pwsh
# pane so it runs even without WSL.
#
# A native shell moves the Win32 cwd on every `cd`, so that reading is always
# fresh and stays authoritative for this pane.  An OSC announcement only
# outranks it when a VT bridge (wsl/ssh) is in the tree and the Win32 reading is
# therefore known to be frozen.  Part D pins BOTH halves: that the sequences are
# accepted at all, and that accepting them did not let a stale announcement
# hijack an ordinary pwsh pane.
# ---------------------------------------------------------------------------
Write-Sect "Part D: OSC 7 / OSC 9;9 acceptance, and no regression for native shells"
$SD = "i615_osc"
New-Sess $SD "C:\"
$host7 = $env:COMPUTERNAME

function Emit-Osc([string]$t, [string]$body) {
    SK $t "`$e=[char]27; [Console]::Out.Write(`"`$e$body`$e\`")"
    Start-Sleep -Milliseconds 900
}

# D1: OSC 7 is recorded (this already worked and must keep working).
Emit-Osc "${SD}:0.0" "]7;file://$host7/mnt/c/Users"
$d1 = PanePath "${SD}:0.0"
if ($d1 -eq "/mnt/c/Users") { Write-Pass "OSC 7 recorded in #{pane_path} as '$d1' (raw, tmux parity)" }
else { Write-Fail "OSC 7 -> #{pane_path} '$d1', expected '/mnt/c/Users'" }

# D2: OSC 9;9 is accepted.  Before the fix psmux ignored it outright and
# #{pane_path} kept its previous value.
Emit-Osc "${SD}:0.0" "]9;9;C:\Windows"
$d2 = PanePath "${SD}:0.0"
if ($d2 -eq "C:\Windows") { Write-Pass "OSC 9;9 accepted -> #{pane_path} '$d2'" }
else { Write-Fail "OSC 9;9 -> #{pane_path} '$d2', expected 'C:\Windows'" }

# D3: OSC 9;9 in ConEmu's quoted form.  The backticks escape the quotes for the
# pwsh running INSIDE the pane, which is what actually builds the byte string.
Emit-Osc "${SD}:0.0" ']9;9;`"C:\Users`"'
$d3 = PanePath "${SD}:0.0"
if ($d3 -eq "C:\Users") { Write-Pass "OSC 9;9 quoted (ConEmu form) -> #{pane_path} '$d3'" }
else { Write-Fail "OSC 9;9 quoted -> #{pane_path} '$d3', expected 'C:\Users'" }

# D4: NO REGRESSION.  There is no bridge in this pane, so the Win32 cwd stays
# authoritative and the announcements above must not have moved
# #{pane_current_path} at all.
$d4 = CurPath "${SD}:0.0"
if ($d4 -eq "C:\") { Write-Pass "native pwsh pane ignores the announcements, still '$d4'" }
else { Write-Fail "native pwsh pane drifted to '$d4' after OSC announcements, expected 'C:\'" }

# D5: and a real `cd` still wins.
SK "${SD}:0.0" 'cd C:\Users\Public'
$d5 = Wait-Path "${SD}:0.0" "C:\Users\Public" 8
if ($d5 -eq "C:\Users\Public") { Write-Pass "native pwsh `cd` still authoritative -> '$d5'" }
else { Write-Fail "after `cd C:\Users\Public` path is '$d5', expected 'C:\Users\Public'" }
Split-AndVerify $SD "C:\Users\Public" "pwsh+osc"
Kill-Sess $SD

# ---------------------------------------------------------------------------
# Part E: the real reporter recipe inside WSL.
# ---------------------------------------------------------------------------
Write-Sect "Part E: WSL (the reported case)"
if (-not $wslDistro) {
    Write-Skip "no usable WSL distro installed"
} else {
    $SE = "i615_wsl"
    New-Sess $SE "C:\"
    $e0 = CurPath "${SE}:0.0"
    if ($e0 -eq "C:\") { Write-Pass "pane starts at '$e0'" } else { Write-Fail "pane starts at '$e0', expected 'C:\'" }

    SK "${SE}:0.0" "wsl -d $wslDistro"
    if (-not (Wait-Cap "${SE}:0.0" '\$' 60)) { Write-Info "wsl prompt not seen within 60s, continuing" }
    Start-Sleep -Seconds 3

    # E1: with NO shell integration psmux cannot know the Linux cwd.  tmux is
    # in the same position when osdep_get_cwd fails: keep the last known path
    # rather than invent one.  Assert the documented fallback, not a wrong
    # value dressed up as right.
    SK "${SE}:0.0" "cd /mnt/c/Users"
    Start-Sleep -Seconds 3
    $e1 = CurPath "${SE}:0.0"
    Write-Info "WSL cd WITHOUT shell integration -> pane_current_path='$e1' (Win32 cwd of wsl.exe never moves)"
    if ($e1 -eq "C:\") { Write-Pass "no shell integration: falls back to the last known path '$e1' (documented)" }
    else { Write-Fail "no shell integration: got '$e1', expected the C:\ fallback" }

    # E2: enable OSC 7 shell integration, exactly the line the docs give the
    # user, then repeat the reporter's recipe.
    SK "${SE}:0.0" 'PROMPT_COMMAND=''printf "\033]7;file://%s%s\033\\" "$HOSTNAME" "$PWD"'''
    Start-Sleep -Seconds 2
    SK "${SE}:0.0" "cd /mnt/c/Users"
    $e2 = Wait-Path "${SE}:0.0" "C:\Users" 8
    if ($e2 -eq "C:\Users") { Write-Pass "WSL + OSC 7: `cd /mnt/c/Users` -> '$e2'" }
    else { Write-Fail "WSL + OSC 7: `cd /mnt/c/Users` -> '$e2', expected 'C:\Users'" }

    # E3: the reporter's actual binding.
    Split-AndVerify $SE "C:\Users" "wsl"

    # E4: a Linux-only path has no drive-letter form; the UNC view of the
    # distro is the only thing Windows can open.
    SK "${SE}:0.0" "cd ~"
    $wantHome = "\\wsl.localhost\$wslDistro" + ($wslHome -replace '/', '\')
    $e4 = Wait-Path "${SE}:0.0" $wantHome 8
    if ($e4 -eq $wantHome) { Write-Pass "WSL + OSC 7: `cd ~` -> '$e4'" }
    else { Write-Fail "WSL + OSC 7: `cd ~` -> '$e4', expected '$wantHome'" }
    if ($e4 -eq $wantHome -and (Test-Path -LiteralPath $e4)) { Write-Pass "the reported home path '$e4' really is openable from Windows" }
    else { Write-Fail "the reported home path '$e4' does not resolve on the Windows side" }

    # E5: back to /mnt and out of wsl, the Win32 cwd takes over again.
    SK "${SE}:0.0" "cd /mnt/c/Windows"
    $e5 = Wait-Path "${SE}:0.0" "C:\Windows" 8
    if ($e5 -eq "C:\Windows") { Write-Pass "WSL + OSC 7: `cd /mnt/c/Windows` -> '$e5'" }
    else { Write-Fail "WSL + OSC 7: `cd /mnt/c/Windows` -> '$e5', expected 'C:\Windows'" }

    SK "${SE}:0.0" "exit"
    Start-Sleep -Seconds 3
    SK "${SE}:0.0" 'cd C:\Users\Public'
    $e6 = Wait-Path "${SE}:0.0" "C:\Users\Public" 10
    if ($e6 -eq "C:\Users\Public") { Write-Pass "after leaving wsl the Win32 cwd takes over again -> '$e6'" }
    else { Write-Fail "after leaving wsl path is '$e6', expected 'C:\Users\Public'" }
    Kill-Sess $SE
}

# ---------------------------------------------------------------------------
Cleanup
Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " Passed:  $($script:Pass)" -ForegroundColor Green
Write-Host " Failed:  $($script:Fail)" -ForegroundColor $(if ($script:Fail) { "Red" } else { "Green" })
Write-Host " Skipped: $($script:Skip)" -ForegroundColor Yellow
Write-Host "=========================================" -ForegroundColor Cyan
exit $script:Fail
