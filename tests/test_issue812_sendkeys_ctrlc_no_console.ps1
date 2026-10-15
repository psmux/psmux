# test_issue812_sendkeys_ctrlc_no_console.ps1
#
# Issue #812: `send-keys C-c` must interrupt `ping -t` started from a cmd.exe
# pane, whatever launched the psmux server. The reporter started the server
# from a Windows OpenSSH session (plain exec, no TTY), so the server has no
# console of its own. tmux parity: send-keys C-c writes 0x03 to the pty and the
# line discipline raises SIGINT for the foreground job.
#
# The server is started in several launch contexts:
#   console      the CLI is run from this script (it shares our console)
#   detached     CreateProcess with DETACHED_PROCESS (no console at all)
#   detached+pg  DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
#   nowindow     CREATE_NO_WINDOW (a hidden console of its own)
#   nowindow+pg  CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
#   ssh          (opt in) `ssh localhost <psmux> new-session -d ...`, exactly
#                the reporter's context; needs sshd and key auth to localhost
# Each round is a fresh server in its own namespace, because the first C-c a
# server relays happens to clear its own inherited "ignore Ctrl+C" bit, so
# only panes born before that first C-c show the bug. The round starts `ping -t 127.0.0.1` in that pane, checks pane_current_command is
# PING, sends C-c, and checks that the pane's command is back to cmd and that
# no new "Reply from" line appears.
#
# Every server runs in a unique -L namespace and is removed with
# `kill-server` in that namespace only.

param(
    [string]$Bin = $env:PSMUX_TEST_BIN,
    [int]$Rounds = 3,
    [string[]]$Contexts = @('console', 'detached', 'detached+pg', 'nowindow', 'nowindow+pg')
)

$ErrorActionPreference = 'Continue'
# `pwsh -File ... -Contexts a,b` arrives as the single string "a,b"
$Contexts = @($Contexts | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
if (-not $Bin) { $Bin = (Get-Command psmux -ErrorAction Stop).Source }
Write-Host "binary: $Bin"

if (-not ('Issue812Launch' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Issue812Launch {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessW(string app, System.Text.StringBuilder cmd, IntPtr pa, IntPtr ta,
        bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    // Runs cmdline with the given creation flags and no inherited handles,
    // waits up to timeoutMs, returns the exit code (or -1 on timeout).
    public static long Run(string cmdline, uint flags, uint timeoutMs) {
        var si = new STARTUPINFO(); si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi;
        if (!CreateProcessW(null, new System.Text.StringBuilder(cmdline), IntPtr.Zero, IntPtr.Zero,
                false, flags, IntPtr.Zero, null, ref si, out pi))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        long rc = -1;
        if (WaitForSingleObject(pi.hProcess, timeoutMs) == 0) { uint c; GetExitCodeProcess(pi.hProcess, out c); rc = c; }
        CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
        return rc;
    }
}
'@
}

$DETACHED_PROCESS = 0x8
$CREATE_NEW_PROCESS_GROUP = 0x200
$CREATE_NO_WINDOW = 0x08000000
$flagsFor = @{
    'detached'    = $DETACHED_PROCESS
    'detached+pg' = $DETACHED_PROCESS -bor $CREATE_NEW_PROCESS_GROUP
    'nowindow'    = $CREATE_NO_WINDOW
    'nowindow+pg' = $CREATE_NO_WINDOW -bor $CREATE_NEW_PROCESS_GROUP
}

# ConsoleFlags bit 0 ("ignore Ctrl+C") of the server and the pane shell, read
# from outside with tests\ctrlflags.cs. Diagnostic only; the verdict is the
# pane's behaviour.
$script:CtrlFlagsExe = "$env:TEMP\psmux_812_ctrlflags.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if ((Test-Path $csc) -and -not (Test-Path $script:CtrlFlagsExe)) {
    & $csc /nologo /optimize /out:$script:CtrlFlagsExe "$PSScriptRoot\ctrlflags.cs" 2>&1 | Out-Null
}
function Get-IgnoreBits([string]$ns, [string]$s) {
    if (-not (Test-Path $script:CtrlFlagsExe)) { return '' }
    $srv = (& $Bin -L $ns display-message -p -t $s '#{pid}' 2>&1 | Out-String).Trim()
    $pane = (& $Bin -L $ns display-message -p -t $s '#{pane_pid}' 2>&1 | Out-String).Trim()
    $o = & $script:CtrlFlagsExe $srv $pane 2>&1
    $bits = $o | ForEach-Object { if ($_ -match 'ignoreCtrlC=(\w+)') { $Matches[1] } else { '?' } }
    return "ignoreCtrlC server=$($bits[0]) pane=$($bits[1])"
}

$defaultBefore = (& $Bin ls 2>&1 | Out-String).Trim()
$pass = 0; $fail = 0
$results = @()

function Count-Replies([string]$ns, [string]$s) {
    $txt = (& $Bin -L $ns capture-pane -p -t $s -S -200 2>&1 | Out-String)
    return ([regex]::Matches($txt, 'Reply from 127\.0\.0\.1')).Count
}

function Wait-Cmd([string]$ns, [string]$s, [string]$want, [int]$ms) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do {
        $c = (& $Bin -L $ns display-message -p -t $s '#{pane_current_command}' 2>&1 | Out-String).Trim()
        if ($c -match $want) { return $c }
        Start-Sleep -Milliseconds 200
    } while ($sw.ElapsedMilliseconds -lt $ms)
    return $c
}

foreach ($ctx in $Contexts) {
    $ctxPass = 0
    for ($r = 1; $r -le $Rounds; $r++) {
        $ns = "t812_" + ($ctx -replace '\W', '') + "_" + (Get-Random -Maximum 99999)
        $s = 'clit'
        if ($ctx -eq 'console') {
            & $Bin -L $ns new-session -d -s $s -x 100 -y 30 cmd.exe
        } elseif ($ctx -eq 'ssh') {
            ssh -o BatchMode=yes -o ConnectTimeout=5 localhost "`"$Bin`" -L $ns new-session -d -s $s -x 100 -y 30 cmd.exe" 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) { Write-Host "  [ssh] ssh to localhost failed (exit $LASTEXITCODE)" }
        } else {
            $rc = [Issue812Launch]::Run("`"$Bin`" -L $ns new-session -d -s $s -x 100 -y 30 cmd.exe",
                [uint32]$flagsFor[$ctx], 15000)
            if ($rc -ne 0) { Write-Host "  [$ctx] launcher exit $rc" }
        }
        $ok = $false
        for ($i = 0; $i -lt 50; $i++) {
            & $Bin -L $ns has-session -t $s 2>$null
            if ($LASTEXITCODE -eq 0) { $ok = $true; break }
            Start-Sleep -Milliseconds 200
        }
        if (-not $ok) { Write-Host "  [$ctx] round $r FAIL: session never came up"; $fail++; continue }
        Wait-Cmd $ns $s '(?i)^cmd' 5000 | Out-Null
        Start-Sleep -Milliseconds 800

        $bits = Get-IgnoreBits $ns $s
        & $Bin -L $ns send-keys -t $s -l 'ping -t 127.0.0.1'
        & $Bin -L $ns send-keys -t $s Enter
        $before = Wait-Cmd $ns $s '(?i)ping' 6000
        Start-Sleep -Milliseconds 1500
        & $Bin -L $ns send-keys -t $s C-c
        $sk = $LASTEXITCODE
        $after = Wait-Cmd $ns $s '(?i)^cmd' 3000
        $n1 = Count-Replies $ns $s
        Start-Sleep -Milliseconds 2500
        $n2 = Count-Replies $ns $s
        $stopped = ($after -match '(?i)^cmd') -and ($n2 -eq $n1)
        $line = "  [$ctx] round $r : before='$before' send-keys rc=$sk after='$after' replies $n1 -> $n2 ($bits)"
        if ($before -notmatch '(?i)ping') {
            Write-Host "$line  SETUP FAIL (ping never became the pane command)"; $fail++
        } elseif ($stopped) {
            Write-Host "$line  PASS"; $pass++; $ctxPass++
        } else {
            Write-Host "$line  FAIL"; $fail++
            if ($r -eq 1) {
                Write-Host "    capture-pane tail:"
                (& $Bin -L $ns capture-pane -p -t $s 2>&1 | Where-Object { $_ -ne '' } | Select-Object -Last 3) |
                    ForEach-Object { Write-Host "      $_" }
            }
        }
        & $Bin -L $ns kill-server 2>$null
    }
    $results += "$ctx : $ctxPass/$Rounds"
}

$defaultAfter = (& $Bin ls 2>&1 | Out-String).Trim()
if ($defaultAfter -ne $defaultBefore) { Write-Host "WARNING: default namespace changed:`n$defaultBefore`n---`n$defaultAfter" }

Write-Host ""
$results | ForEach-Object { Write-Host "  $_" }
Write-Host "Results: $pass passed, $fail failed"
exit $(if ($fail -eq 0) { 0 } else { 1 })
