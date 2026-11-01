# run-shell -b from the CLI belongs to the server, as in tmux (cmd-run-shell.c:187)
#
# Before: the CLI spawned a -b job as its own child holding the CLI's stdout,
# so `psmux run-shell -b "helper.exe 2"` read from PowerShell took the whole
# 2 s and printed the job's output. Also tmux's `-c start-directory` went to
# the shell as text.
#
# Isolated: a unique -L namespace and a private PSMUX_DATA_DIR. Cleanup only
# through `-L <ns> kill-server`; nothing is killed by name.
param([string]$Exe)

$ErrorActionPreference = 'Continue'
if (-not $Exe) { $Exe = (Get-Command psmux -EA Stop).Source }
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

$ns = "rshb_$PID"
$work = Join-Path $env:TEMP "psmux_rshb_$PID"
$data = Join-Path $work 'data'
New-Item -ItemType Directory $data -Force | Out-Null
$savedData = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $data

# A tiny native probe: records its pid, prints a line, sleeps, then records
# that it finished.
$probe = Join-Path $work 'probe.exe'
$src = Join-Path $work 'probe.cs'
@'
using System; using System.IO; using System.Threading;
class P { static void Main(string[] a) {
  string dir = a[0]; string tag = a[1]; int ms = int.Parse(a[2]);
  File.WriteAllText(Path.Combine(dir, tag + ".start"), Directory.GetCurrentDirectory());
  Console.WriteLine("PROBE_STDOUT_" + tag);
  Thread.Sleep(ms);
  File.WriteAllText(Path.Combine(dir, tag + ".done"), "ok");
} }
'@ | Set-Content $src -Encoding ASCII
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) 'csc.exe'
if (-not (Test-Path $csc)) { $csc = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe' }
& $csc /nologo /out:$probe $src | Out-Null
if (-not (Test-Path $probe)) { Write-Host 'probe did not compile'; exit 1 }

function WaitFile($p, $ms = 8000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (Test-Path $p) { return $true }; Start-Sleep -Milliseconds 50 }
    return $false
}

try {
    & $Exe -L $ns new-session -d -s t
    Start-Sleep -Milliseconds 1500
    & $Exe -L $ns has-session -t t 2>$null
    if ($LASTEXITCODE -ne 0) { Fail 'session did not start'; exit 1 }

    Write-Host "`n[1] -b returns at once and prints nothing" -ForegroundColor Yellow
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = & $Exe -L $ns run-shell -b "$probe $work bg 3000" 2>&1
    $ms = $sw.ElapsedMilliseconds
    if ($ms -lt 1500) { Pass "returned in $ms ms (job sleeps 3000)" } else { Fail "CLI waited for the job: $ms ms" }
    if (-not ($out | Out-String).Contains('PROBE_STDOUT')) { Pass 'job stdout did not reach the CLI' } else { Fail "job stdout on the CLI: $out" }
    if (WaitFile "$work\bg.start") { Pass 'job started' } else { Fail 'job never started' }
    if (WaitFile "$work\bg.done") { Pass 'job ran to the end after the CLI exited' } else { Fail 'job did not finish' }

    Write-Host "`n[2] -b with the CLI's stdout piped to a reader that closes" -ForegroundColor Yellow
    cmd /c "`"$Exe`" -L $ns run-shell -b `"$probe $work piped 1500`" | exit 0"
    if ((WaitFile "$work\piped.start") -and (WaitFile "$work\piped.done")) { Pass 'job survived the closed pipe' } else { Fail 'job did not survive the closed pipe' }

    Write-Host "`n[3] -b without -c uses the caller's directory" -ForegroundColor Yellow
    $here = Join-Path $work 'here dir'
    New-Item -ItemType Directory $here -Force | Out-Null
    Push-Location $here
    & $Exe -L $ns run-shell -b "$probe $work cwd 10" 2>&1 | Out-Null
    Pop-Location
    if (WaitFile "$work\cwd.start") {
        $got = (Get-Content "$work\cwd.start" -Raw).Trim()
        if ($got -eq $here) { Pass "job ran in the caller's directory" } else { Fail "cwd '$got', caller '$here'" }
    } else { Fail 'cwd job never started' }

    Write-Host "`n[4] -c start-directory on the CLI, the server route and a binding" -ForegroundColor Yellow
    $cdir = Join-Path $work 'c dir'
    New-Item -ItemType Directory $cdir -Force | Out-Null
    & $Exe -L $ns run-shell -c $cdir "$probe $work cfg 10" 2>&1 | Out-Null
    if ((WaitFile "$work\cfg.start") -and ((Get-Content "$work\cfg.start" -Raw).Trim() -eq $cdir)) { Pass 'CLI foreground -c' } else { Fail "CLI foreground -c: $(Get-Content "$work\cfg.start" -EA SilentlyContinue)" }
    & $Exe -L $ns run-shell -b -c $cdir "$probe $work cbg 10" 2>&1 | Out-Null
    if ((WaitFile "$work\cbg.start") -and ((Get-Content "$work\cbg.start" -Raw).Trim() -eq $cdir)) { Pass 'CLI -b -c' } else { Fail "CLI -b -c: $(Get-Content "$work\cbg.start" -EA SilentlyContinue)" }
    # A format sends the command through the server; the probe ignores the
    # fourth word.
    & $Exe -L $ns run-shell -c $cdir "$probe $work cfmt 10 #{session_name}" 2>&1 | Out-Null
    if ((WaitFile "$work\cfmt.start") -and ((Get-Content "$work\cfmt.start" -Raw).Trim() -eq $cdir)) { Pass 'server route (format) -c' } else { Fail "server route -c: $(Get-Content "$work\cfmt.start" -EA SilentlyContinue)" }
    & $Exe -L $ns set-hook -g after-new-window "run-shell -b -c '$cdir' '$probe $work chook 10'"
    & $Exe -L $ns new-window -d
    if ((WaitFile "$work\chook.start") -and ((Get-Content "$work\chook.start" -Raw).Trim() -eq $cdir)) { Pass 'hook -c' } else { Fail "hook -c: $(Get-Content "$work\chook.start" -EA SilentlyContinue)" }
    & $Exe -L $ns set-hook -gu after-new-window

    Write-Host "`n[5] foreground run-shell still prints and waits" -ForegroundColor Yellow
    $out = (& $Exe -L $ns run-shell "$probe $work fg 300" 2>&1 | Out-String)
    if ($out.Contains('PROBE_STDOUT_fg')) { Pass 'foreground output printed' } else { Fail "foreground output missing: $out" }
    if (Test-Path "$work\fg.done") { Pass 'foreground waited for the job' } else { Fail 'foreground did not wait' }


}
finally {
    & $Exe -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $env:PSMUX_DATA_DIR = $savedData
    Remove-Item -Recurse -Force $work -EA SilentlyContinue
}

Write-Host "`nPassed: $script:Passed  Failed: $script:Failed"
exit $script:Failed
