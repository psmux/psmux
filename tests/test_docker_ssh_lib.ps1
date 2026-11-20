# Shared helpers for the psmux docker environment tests (dot-source this file).
#
# The docker env (docker\Run-PsmuxDev.ps1) is a windowsservercore-ltsc2022
# container (Windows build 20348, ConPTY WITHOUT mouse support) running
# OpenSSH on port 2222 with key-only auth. psmux must be built and installed
# inside the container (cargo install --path .) before the tests run.
#
# Every helper here talks to the container from the HOST:
#   Invoke-CSsh          non-interactive SSH exec (remote pwsh, no TTY)
#   Copy-ToContainer     scp a local file into the container
#   Start-InteractiveSsh ssh -tt with redirected stdio = REAL interactive
#                        attach through a remote ConPTY; stdin bytes are
#                        keystrokes, stdout bytes are the raw VT stream the
#                        user's terminal would receive
#
# The interactive harness is the only way to exercise the SSH-input module
# (SSH_TTY path), the TUI renderer, and the prefix-key handling exactly the
# way a real "ssh -p 2222 ... psmux attach" user would.

# KNOWN ENV ISSUE (2026-07): pwsh 7.5 crashes with 0xc0000005 when spawned
# under a ConPTY inside this hyperv container (also crashes under plain
# docker exec), so sessions here must use Windows PowerShell 5.1 or cmd as
# the pane shell. pwsh works fine under sshd's ConPTY (ssh -tt login shell).
# PowerShell tracks the crash as PowerShell/PowerShell#25541 (closed as External).
$script:PaneShell  = "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"

$script:DockerKey  = Join-Path $env:USERPROFILE ".ssh\psmux_docker_key"
$script:SshUser    = "ContainerAdministrator"
$script:SshPort    = 2222
$script:ContainerName = "psmux-dev"

function Get-DockerExe {
    $cmd = Get-Command docker -EA SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $desktop = "C:\Program Files\Docker\Docker\resources\bin\docker.exe"
    if (Test-Path $desktop) { return $desktop }
    # Missing docker = missing prerequisite, as in test_docker_exec_lib.ps1:
    # skip (exit 0) instead of throwing, which the runner counted as a FAIL.
    Write-Host "[SKIP] docker CLI not found - docker suites need docker\Run-PsmuxDev.ps1 environment" -ForegroundColor Yellow
    exit 0
}

function Get-ContainerIP {
    param([string]$Name = $script:ContainerName)
    $docker = Get-DockerExe
    $state = (& $docker inspect $Name --format "{{.State.Running}}" 2>$null)
    if ($state -ne "true") {
        & $docker start $Name 2>&1 | Out-Null
        Start-Sleep -Seconds 5
    }
    $ip = (& $docker inspect $Name --format "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}" 2>$null)
    if (-not $ip) {
        # Missing container = missing prerequisite, not a psmux defect: skip
        # the suite cleanly so unattended sweeps count only real failures.
        Write-Host "[SKIP] container '$Name' has no IP - run docker\Run-PsmuxDev.ps1 first" -ForegroundColor Yellow
        exit 0
    }
    return $ip.Trim()
}

function Get-SshArgs {
    param([string]$Ip)
    return @(
        "-i", $script:DockerKey,
        "-p", $script:SshPort,
        "-o", "BatchMode=yes",
        "-o", "LogLevel=ERROR",
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=NUL",
        "-o", "ConnectTimeout=15",
        "$($script:SshUser)@$Ip"
    )
}

# Non-interactive remote exec (no TTY). Remote default shell is pwsh.
# Returns stdout as string; remote exit code in $script:CSshExit.
function Invoke-CSsh {
    param([string]$Ip, [string]$Command)
    $out = & ssh @(Get-SshArgs $Ip) $Command 2>&1 | Out-String
    $script:CSshExit = $LASTEXITCODE
    return $out
}

function Copy-ToContainer {
    param([string]$Ip, [string]$LocalPath, [string]$RemotePath)
    & scp -i $script:DockerKey -P $script:SshPort -o StrictHostKeyChecking=no -o UserKnownHostsFile=NUL `
        $LocalPath "$($script:SshUser)@${Ip}:$RemotePath" 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

# ---------------- interactive attach harness (ssh -tt) ----------------

function Start-InteractiveSsh {
    param([string]$Ip, [string]$RemoteCommand)
    $sshExe = (Get-Command ssh -EA Stop).Source
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $sshExe
    $args = @("-tt") + (Get-SshArgs $Ip) + @($RemoteCommand)
    $psi.Arguments = ($args | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join ' '
    $psi.RedirectStandardInput  = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError  = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow  = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    $out = [System.IO.MemoryStream]::new()
    $err = [System.IO.MemoryStream]::new()
    $outTask = $proc.StandardOutput.BaseStream.CopyToAsync($out)
    $errTask = $proc.StandardError.BaseStream.CopyToAsync($err)
    return @{ Proc = $proc; Out = $out; Err = $err; OutTask = $outTask; ErrTask = $errTask }
}

# Snapshot of everything the remote side sent so far (raw VT stream).
function Get-StreamText {
    param($Sess)
    try { return [System.Text.Encoding]::UTF8.GetString($Sess.Out.ToArray()) } catch { return "" }
}

function Wait-StreamMatch {
    param($Sess, [string]$Pattern, [int]$TimeoutSec = 15)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        $txt = Get-StreamText $Sess
        if ($txt -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

# Send raw bytes as keystrokes into the remote pty (0x02 = Ctrl+B prefix).
function Send-RawBytes {
    param($Sess, [byte[]]$Bytes)
    $Sess.Proc.StandardInput.BaseStream.Write($Bytes, 0, $Bytes.Length)
    $Sess.Proc.StandardInput.BaseStream.Flush()
}

function Send-Text {
    param($Sess, [string]$Text)
    Send-RawBytes $Sess ([System.Text.Encoding]::UTF8.GetBytes($Text))
}

function Stop-InteractiveSsh {
    param($Sess)
    try { if (-not $Sess.Proc.HasExited) { $Sess.Proc.Kill() } } catch {}
    try { $Sess.Proc.Dispose() } catch {}
}

# Wait for the attached TUI to render (pattern in the raw stream). The first
# status-bar frame can lag over SSH, so nudge a redraw with a harmless Enter
# keystroke if it has not appeared after the first wait window.
function Wait-TuiRender {
    param($Sess, [string]$Pattern, [int]$TimeoutSec = 40)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (Wait-StreamMatch $Sess $Pattern 8) { return $true }
        Send-Text $Sess "`r"
    }
    return $false
}

# Create a detached session in the container with a shell that survives this
# environment (see PaneShell note above), and make new windows inherit it.
function New-ContainerSession {
    param([string]$Ip, [string]$Name)
    Invoke-CSsh $Ip "psmux new-session -d -s $Name -- $($script:PaneShell)" | Out-Null
    Start-Sleep -Seconds 3
    Invoke-CSsh $Ip "psmux set-option -g default-shell $($script:PaneShell)" | Out-Null
    # wait for the pane shell to reach its prompt so attaches render promptly
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $cap = Invoke-CSsh $Ip "psmux capture-pane -t $Name -p"
        if ($cap -match "PS C:") { return }
        Start-Sleep -Seconds 2
    }
}
