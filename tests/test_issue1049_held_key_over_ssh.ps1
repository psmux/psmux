# Issue #1049 over the reporter's real path: a held key behind ssh.
#
# tests\test_issue1049_held_key_not_a_paste.ps1 emulates the shape with
# WriteConsoleInputW clumps. This file drives the path the reporter uses, end
# to end: a REAL Unix ssh client (WSL) under a REAL pty writes into Windows
# sshd, whose ConPTY hosts the attached psmux client, and the pane child is
# tests\paste_recorder.cs, so the assertion is on the bytes the pane read.
#
#   1. a held `j` whose repeats the network bunches: 30 writes of `jjjj`,
#      133 ms apart. Every repeat must reach the pane as a key, never inside
#      ESC[200~ ... ESC[201~ (master: 30 bracketed pastes of `jjjj`).
#   2. the control: 60 `j` written one at a time, 33 ms apart.
#   3. the guard: real bracketed pastes from the outer terminal, including a
#      pasted run of one character, still arrive bracketed and whole.
#
# Skips cleanly, exit 0, when WSL, a WSL python3, sshd on localhost or key auth
# for the current user is missing. Honours PSMUX_TEST_BIN; uses its own -L
# namespace and PSMUX_DATA_DIR.
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i1049ssh_" + (Get-Random -Maximum 999999)
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }
function Write-Skip($m) { Write-Host "[SKIP] $m" -ForegroundColor Yellow }

Write-Host "=== Issue #1049: a held key behind a real ssh connection ===" -ForegroundColor Cyan
Write-Info "binary: $PSMUX"

# --- preflight ---------------------------------------------------------------
if (-not (Get-Command wsl.exe -EA SilentlyContinue)) { Write-Skip "wsl.exe not present"; exit 0 }
$distro = if ($env:PSMUX_TEST_WSL_DISTRO) { $env:PSMUX_TEST_WSL_DISTRO } else { "Ubuntu" }
$probe = & wsl -d $distro -e bash -lc "command -v python3 >/dev/null && echo PY_OK" 2>&1 | Out-String
if ($probe -notmatch "PY_OK") { Write-Skip "WSL distro '$distro' or its python3 is unavailable"; exit 0 }
$sshd = Get-Service sshd -EA SilentlyContinue
if (-not $sshd -or $sshd.Status -ne "Running") { Write-Skip "Windows sshd service is not running"; exit 0 }
$sshUser = if ($env:PSMUX_TEST_SSH_USER) { $env:PSMUX_TEST_SSH_USER } else { $env:USERNAME }
$keyWin = Join-Path $env:USERPROFILE ".ssh\id_ed25519"
if (-not (Test-Path $keyWin)) { Write-Skip "no $keyWin to give the WSL ssh client"; exit 0 }
$selfProbe = & ssh -o ConnectTimeout=5 -o BatchMode=yes "$sshUser@localhost" "echo PSMUX_SSH_OK" 2>$null
if ("$selfProbe" -notmatch "PSMUX_SSH_OK") { Write-Skip "key auth to $sshUser@localhost is not configured"; exit 0 }
$hostIp = (& wsl -d $distro -e bash -lc "ip route show default | awk '{print `$3}'" 2>&1 | Out-String).Trim()
if (-not $hostIp) { Write-Skip "could not determine the Windows host address from WSL"; exit 0 }

foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM

$root = Join-Path $env:TEMP ("psmux_i1049_ssh_" + (Get-Random -Maximum 999999))
New-Item -ItemType Directory -Force $root | Out-Null
$dataDir = Join-Path $root "data"
New-Item -ItemType Directory -Force $dataDir | Out-Null
$env:PSMUX_DATA_DIR = $dataDir
$env:PSMUX_NO_WARM = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$recorder = Join-Path $root "paste_recorder.exe"
& $csc /nologo /optimize /platform:x64 /out:$recorder (Join-Path $PSScriptRoot "paste_recorder.cs") 2>&1 | Out-Null
if (-not (Test-Path $recorder)) { Write-Host "FATAL: could not compile the recorder" -ForegroundColor Red; exit 1 }

# The sshd side gets its own environment, so the data dir travels in the command.
$attachCmd = Join-Path $root "attach.cmd"
@('@echo off', "set PSMUX_DATA_DIR=$dataDir", '"%~1" -L %~2 attach -t %~3') -join "`r`n" | Set-Content -Encoding ASCII $attachCmd

# The pty driver: runs a list of "<wait seconds> <hex bytes>" steps.
$driver = @'
import os, sys, pty, select, time, fcntl, termios, struct, signal, subprocess
hostip, keyfile, remote, outlog, stepsfile, user = sys.argv[1:7]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
env = dict(os.environ); env['TERM'] = 'xterm-256color'
args = ['ssh', '-tt', '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null',
        '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', '-i', keyfile, user + '@' + hostip, remote]
p = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave, preexec_fn=os.setsid, env=env, close_fds=True)
os.close(slave)
out = open(outlog, 'wb')
def pump(sec):
    end = time.time() + sec
    while True:
        r, _, _ = select.select([master], [], [], max(0, min(0.02, end - time.time())))
        if master in r:
            try: d = os.read(master, 65536)
            except OSError: return
            if not d: return
            out.write(d); out.flush()
        if time.time() >= end: return
for line in open(stepsfile).read().split('\n'):
    line = line.strip()
    if not line: continue
    parts = line.split(' ', 1)
    pump(float(parts[0]))
    if len(parts) > 1 and parts[1]:
        os.write(master, bytes.fromhex(parts[1]))
pump(2.0)
try: os.killpg(os.getpgid(p.pid), signal.SIGTERM)
except Exception: pass
try: p.wait(timeout=5)
except Exception:
    try: os.killpg(os.getpgid(p.pid), signal.SIGKILL)
    except Exception: pass
out.close()
print("DRIVER_DONE")
'@
$driverWin = Join-Path $root "drv.py"
Set-Content -Encoding ASCII -Path $driverWin -Value $driver

function To-WslPath([string]$p) { "/mnt/" + $p.Substring(0,1).ToLower() + $p.Substring(2).Replace('\','/') }
$tag = Get-Random -Maximum 999999
$keyWsl = "/tmp/psmux_i1049ssh_key_$tag"
$drvWsl = "/tmp/psmux_i1049ssh_drv_$tag.py"
$prep = "cp '$(To-WslPath $keyWin)' $keyWsl && sed -i 's/\r`$//' $keyWsl && chmod 600 $keyWsl && sed 's/\r`$//' '$(To-WslPath $driverWin)' > $drvWsl && echo PREP_OK"
$prepOut = & wsl -d $distro -e bash -lc $prep 2>&1 | Out-String
if ($prepOut -notmatch "PREP_OK") { Write-Skip "could not stage the ssh key inside WSL: $prepOut"; exit 0 }

$ESC = [char]0x1b
function Hx([string]$s) { ([Text.Encoding]::UTF8.GetBytes($s) | ForEach-Object { $_.ToString("x2") }) -join "" }

# One attached client behind ssh, the recorder as the pane child, the steps.
function Invoke-Scenario([string]$name, [string[]]$steps) {
    $sess = "s_$name"
    $log = Join-Path $root "rec_$name.log"
    $stop = Join-Path $root "stop_$name"
    & $PSMUX -L $NS new-session -d -s $sess -x 120 -y 40 -- $recorder $log 120 vt $stop 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $stepsFile = Join-Path $root "$name.steps"
    ((@("6") + $steps + @("3")) -join "`n") | Set-Content -Encoding ASCII -NoNewline $stepsFile
    $outLog = Join-Path $root "$name.ssh.bin"
    $remote = "$attachCmd `"$PSMUX`" $NS $sess"
    $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($remote))
    $cmd = "R=`$(echo $b64 | base64 -d); python3 $drvWsl '$hostIp' '$keyWsl' `"`$R`" " +
           "'$(To-WslPath $outLog)' '$(To-WslPath $stepsFile)' '$sshUser'"
    $drv = & wsl -d $distro -e bash -lc $cmd 2>&1 | Out-String
    if ($drv -notmatch "DRIVER_DONE") { Write-Info "driver output: $drv" }
    New-Item -ItemType File $stop -Force | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^TEXT ' -Quiet)) -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 300 }
    $res = [ordered]@{ Text = ""; Has200 = "?" }
    foreach ($l in Get-Content $log -EA SilentlyContinue) {
        if ($l -match '^TEXT (.*)$') { $res.Text = $Matches[1] }
        elseif ($l -match '^HAS200 (\w+)$') { $res.Has200 = $Matches[1] }
    }
    & $PSMUX -L $NS kill-session -t $sess 2>&1 | Out-Null
    [pscustomobject]$res
}

try {
    Write-Host "`n[Test 1] held j, repeats bunched four to a write by the network" -ForegroundColor Yellow
    $r = Invoke-Scenario "held" @(1..30 | ForEach-Object { "0.133 " + (Hx "jjjj") })
    if ($r.Has200 -eq "?") { Write-Fail "the recorder wrote no log (client never attached?)" }
    else {
        if ($r.Text -eq ("j" * 120)) { Write-Pass "all 120 repeats reached the pane as keys" }
        else { Write-Fail "the held key arrived as '$(if ($r.Text.Length -gt 120) { $r.Text.Substring(0,120) + '...' } else { $r.Text })'" }
        if ($r.Has200 -eq "NO") { Write-Pass "no repeat was wrapped in bracketed paste" }
        else { Write-Fail "the held key was bracketed (the #1049 symptom)" }
    }

    Write-Host "`n[Test 2] control: held j, one repeat per write, 33 ms apart" -ForegroundColor Yellow
    $r = Invoke-Scenario "even" @(1..60 | ForEach-Object { "0.033 " + (Hx "j") })
    if ($r.Text -eq ("j" * 60) -and $r.Has200 -eq "NO") { Write-Pass "60 keys, no paste" }
    else { Write-Fail "even repeats arrived as '$($r.Text)' (200=$($r.Has200))" }

    Write-Host "`n[Test 3] guard: real bracketed pastes from the outer terminal" -ForegroundColor Yellow
    $rule = "-" * 40
    # The first paste waits 2 s, not 0.5 s: until the client has put its
    # console on VT input, conhost strips the paste markers and hands over
    # plain keys, and a run of one character is then a held key (#1049).
    # A loaded machine (run 2026-11-22_17-07-40) attached later than 0.5 s.
    $r = Invoke-Scenario "paste" @(
        ("2 " + (Hx "$ESC[200~$rule$ESC[201~")),
        ("1 " + (Hx "$ESC[200~jjjjjjjj$ESC[201~")),
        ("1 " + (Hx "$ESC[200~hello world$ESC[201~")))
    $want = "<ESC>[200~$rule<ESC>[201~<ESC>[200~jjjjjjjj<ESC>[201~<ESC>[200~hello world<ESC>[201~"
    if ($r.Text -eq $want) { Write-Pass "three pastes, each whole and bracketed, a pasted run of one character included" }
    else { Write-Fail "the pastes changed shape: '$($r.Text)'" }
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    & wsl -d $distro -e bash -lc "rm -f $keyWsl $drvWsl" 2>&1 | Out-Null
    if ($null -eq $savedDataDir) { Remove-Item env:PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $savedDataDir }
    if ($null -eq $savedNoWarm) { Remove-Item env:PSMUX_NO_WARM -EA SilentlyContinue } else { $env:PSMUX_NO_WARM = $savedNoWarm }
}

Write-Host "`n=== Results: $script:Pass passed, $script:Fail failed ===" -ForegroundColor $(if ($script:Fail -eq 0) { "Green" } else { "Red" })
exit $(if ($script:Fail -eq 0) { 0 } else { 1 })
