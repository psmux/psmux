# Keystroke to screen latency benchmark.
#
# One command, every scenario, distributions not single numbers. Everything is
# measured by tests/keylat.cs, which injects a key record into a console input
# buffer and then watches that same console's screen buffer for the echo, both
# timestamped from one QueryPerformanceCounter inside one process.
#
# The point of the baselines is that psmux is measured EXACTLY the way a bare
# console is: same injector, same oracle, same timer. The difference between the
# two rows IS psmux's keystroke overhead.
#
#   pwsh -NoProfile -File tests\test_keystroke_latency_bench.ps1
#   pwsh -NoProfile -File tests\test_keystroke_latency_bench.ps1 -Scenarios core
#   pwsh -NoProfile -File tests\test_keystroke_latency_bench.ps1 -N 60 -Tag after
#
# Scenario groups: core (psmux vs bare, single keystroke), typing (sustained and
# burst), load (heavy pane output), baselines (windows terminal), tui (nvim),
# all (default).

param(
    [string]$Scenarios = "all",
    [int]$N = 40,
    [string]$Tag = "run",
    [string]$Psmux = "",
    [string]$OutDir = "",
    # A/B the timer-resolution fix against the SAME binary in the same time
    # window, which is the only honest comparison on a machine this busy.
    [switch]$NoTimerRes
)

if ($NoTimerRes) { $env:PSMUX_NO_TIMER_RES = "1" } else { Remove-Item Env:\PSMUX_NO_TIMER_RES -EA SilentlyContinue }

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Psmux) { $Psmux = Join-Path $root "target\release\psmux.exe" }
$KeyLat = Join-Path $root "target\release\keylat.exe"
$EchoChild = Join-Path $root "target\release\echo_load_child.exe"
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP "psmux_keylat" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
foreach ($pair in @(@("keylat", $KeyLat), @("echo_load_child", $EchoChild))) {
    $src = Join-Path $PSScriptRoot "$($pair[0]).cs"
    if ((-not (Test-Path $pair[1])) -or ((Get-Item $src).LastWriteTime -gt (Get-Item $pair[1]).LastWriteTime)) {
        & $csc /nologo /optimize "/out:$($pair[1])" $src | Out-Null
    }
}
foreach ($f in @($Psmux, $KeyLat, $EchoChild)) {
    if (-not (Test-Path $f)) { Write-Host "MISSING $f" -ForegroundColor Red; exit 1 }
}

$NS = "keylat$PID"
$script:Results = @()
$script:Procs = @()

function Stop-Bench {
    & $Psmux -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    foreach ($p in $script:Procs) {
        try { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } } catch {}
    }
    $script:Procs = @()
}

# Close the Windows Terminal window this suite opened, found by the unique tab
# title it was given (the window title is the active tab's, possibly prefixed
# "Administrator: "). Waits for Terminal to close it by itself first. WM_CLOSE
# on a one tab window asks nothing, and nothing is ever typed into it.
function Close-OwnWtWindow([string]$title) {
    if (-not ('KeylatWt' -as [type])) {
        Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class KeylatWt {
    delegate bool P(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(P cb, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    public static List<IntPtr> Find(string title) {
        var r = new List<IntPtr>();
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            var c = new StringBuilder(256); GetClassNameW(h, c, 256);
            if (c.ToString() != "CASCADIA_HOSTING_WINDOW_CLASS") return true;
            var t = new StringBuilder(512); GetWindowTextW(h, t, 512);
            string s = t.ToString();
            if (s == title || s.EndsWith(" " + title)) r.Add(h);
            return true;
        }, IntPtr.Zero);
        return r;
    }
}
'@
    }
    for ($i = 0; $i -lt 15; $i++) {
        if ([KeylatWt]::Find($title).Count -eq 0) { return }
        Start-Sleep -Milliseconds 200
    }
    foreach ($h in [KeylatWt]::Find($title)) {
        [void][KeylatWt]::PostMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        Write-Host "  closed the Windows Terminal window this suite opened ($title)" -ForegroundColor DarkGray
    }
}

# Launch an ATTACHED psmux client in its own console window and return the
# process. The client's pid is the console the injector attaches to.
function Start-PsmuxClient([string]$session, [string]$command) {
    $argv = @("-L", $NS, "new-session", "-s", $session)
    if ($command) { $argv += $command }
    $p = Start-Process -FilePath $Psmux -ArgumentList $argv -PassThru
    $script:Procs += $p
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 300
        $ls = & $Psmux -L $NS ls 2>&1 | Out-String
        if ($ls -match [regex]::Escape($session)) { break }
    }
    Start-Sleep -Seconds 3   # let the shell finish drawing its first prompt
    return $p
}

function Start-Bare([string]$exe, [string[]]$argv) {
    $p = Start-Process -FilePath $exe -ArgumentList $argv -PassThru
    $script:Procs += $p
    Start-Sleep -Seconds 4
    return $p
}

# Run keylat against a console and parse its SUMMARY line into an object.
function Invoke-KeyLat([int]$targetPid, [string]$label, [string[]]$extra) {
    $out = Join-Path $OutDir "$Tag`_$label.txt"
    Remove-Item $out -EA SilentlyContinue
    $argv = @("--pid", $targetPid, "--label", $label, "--out", $out) + $extra
    & $KeyLat @argv | Out-Null
    if (-not (Test-Path $out)) {
        Write-Host "  $label : NO OUTPUT" -ForegroundColor Red
        return $null
    }
    $txt = Get-Content $out -Raw
    $m = [regex]::Match($txt, 'SUMMARY \S+ n=(\d+) min=([\d.]+) p25=([\d.]+) median=([\d.]+) mean=([\d.]+) p90=([\d.]+) p99=([\d.]+) max=([\d.]+)')
    if (-not $m.Success) {
        Write-Host "  $label : NO SUMMARY -> $($txt.Trim())" -ForegroundColor Red
        return $null
    }
    $r = [pscustomobject]@{
        Scenario = $label
        N        = [int]$m.Groups[1].Value
        Min      = [double]$m.Groups[2].Value
        Median   = [double]$m.Groups[4].Value
        Mean     = [double]$m.Groups[5].Value
        P90      = [double]$m.Groups[6].Value
        P99      = [double]$m.Groups[7].Value
        Max      = [double]$m.Groups[8].Value
    }
    $script:Results += $r
    Write-Host ("  {0,-34} n={1,-4} min={2,7:F2} med={3,7:F2} p90={4,7:F2} p99={5,7:F2} max={6,7:F2}" -f `
        $r.Scenario, $r.N, $r.Min, $r.Median, $r.P90, $r.P99, $r.Max) -ForegroundColor Green
    $miss = [regex]::Match($txt, 'MISSING \S+ (\d+) of (\d+)')
    if ($miss.Success -and [int]$miss.Groups[1].Value -gt 0) {
        Write-Host "      MISSING $($miss.Groups[1].Value) of $($miss.Groups[2].Value)" -ForegroundColor Yellow
    }
    return $r
}

function Section([string]$name) {
    Write-Host ""
    Write-Host "== $name" -ForegroundColor Cyan
}

$want = { param($g) $Scenarios -eq "all" -or $Scenarios -eq $g }

Stop-Bench

# ---------------------------------------------------------------- core
if (& $want "core") {
    Section "CORE single keystroke at an idle pwsh prompt"
    $p = Start-PsmuxClient "core" "pwsh -NoLogo -NoProfile"
    Invoke-KeyLat $p.Id "psmux_pwsh" @("--mode","pollcost") | Out-Null
    Invoke-KeyLat $p.Id "psmux_pwsh" @("--mode","single","--n","$N","--warmup","5","--gap","120")
    Stop-Bench

    $b = Start-Bare "pwsh" @("-NoLogo","-NoProfile")
    Invoke-KeyLat $b.Id "bare_pwsh" @("--mode","single","--n","$N","--warmup","5","--gap","120")
    Stop-Bench

    Section "CORE single keystroke into a raw echo child (no line editor)"
    $p = Start-PsmuxClient "echo" $EchoChild
    Invoke-KeyLat $p.Id "psmux_echo" @("--mode","single","--n","$N","--warmup","5","--gap","120","--oracle","cell:0,0","--noerase")
    Stop-Bench

    $b = Start-Bare $EchoChild @()
    Invoke-KeyLat $b.Id "bare_echo" @("--mode","single","--n","$N","--warmup","5","--gap","120","--oracle","cell:0,0","--noerase")
    Stop-Bench
}

# ---------------------------------------------------------------- typing
if (& $want "typing") {
    $para = "the quick brown fox jumps over the lazy dog while packing"
    Section "SUSTAINED typing, human speed 10 cps"
    $p = Start-PsmuxClient "type" "pwsh -NoLogo -NoProfile"
    Invoke-KeyLat $p.Id "psmux_type_10cps" @("--mode","type","--cps","10","--text",$para)
    Start-Sleep -Milliseconds 800
    Invoke-KeyLat $p.Id "psmux_type_30cps" @("--mode","type","--cps","30","--text",$para)
    Start-Sleep -Milliseconds 800
    Invoke-KeyLat $p.Id "psmux_type_burst100" @("--mode","type","--cps","100","--text",$para)
    Stop-Bench

    $b = Start-Bare "pwsh" @("-NoLogo","-NoProfile")
    Invoke-KeyLat $b.Id "bare_type_10cps" @("--mode","type","--cps","10","--text",$para)
    Start-Sleep -Milliseconds 800
    Invoke-KeyLat $b.Id "bare_type_30cps" @("--mode","type","--cps","30","--text",$para)
    Start-Sleep -Milliseconds 800
    Invoke-KeyLat $b.Id "bare_type_burst100" @("--mode","type","--cps","100","--text",$para)
    Stop-Bench
}

# ---------------------------------------------------------------- load
if (& $want "load") {
    Section "UNDER LOAD, same pane producing heavy output"
    $p = Start-PsmuxClient "load1" "$EchoChild max 70"
    Invoke-KeyLat $p.Id "psmux_echo_sameload" @("--mode","single","--n","$N","--warmup","5","--gap","120","--oracle","cell:0,0","--noerase")
    Stop-Bench

    $b = Start-Bare $EchoChild @("max","70")
    Invoke-KeyLat $b.Id "bare_echo_sameload" @("--mode","single","--n","$N","--warmup","5","--gap","120","--oracle","cell:0,0","--noerase")
    Stop-Bench

    Section "UNDER LOAD, sibling pane producing heavy output"
    $p = Start-PsmuxClient "load2" "pwsh -NoLogo -NoProfile"
    & $Psmux -L $NS split-window -t "load2" -d "$EchoChild max 70" 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    Invoke-KeyLat $p.Id "psmux_pwsh_siblingload" @("--mode","single","--n","$N","--warmup","5","--gap","120")
    $para2 = "typing while the other pane floods"
    Invoke-KeyLat $p.Id "psmux_type10_siblingload" @("--mode","type","--cps","10","--text",$para2)
    Stop-Bench
}

# ---------------------------------------------------------------- baselines
if (& $want "baselines") {
    Section "BASELINE Windows Terminal hosted pwsh"
    $before = @(Get-Process pwsh -EA SilentlyContinue | Select-Object -ExpandProperty Id)
    $wt = Get-Command wt.exe -EA SilentlyContinue
    if ($wt) {
        # This used to leave one Windows Terminal window behind per run, titled
        # "Administrator: pwsh" and reading "[process exited with code
        # 4294967295 (0xffffffff)]": the pwsh below is force killed, and
        # Terminal keeps a tab whose process failed. Every full sweep added one.
        # So pwsh runs under `cmd /c ... & exit /b 0`, which turns the kill into
        # a clean exit Terminal closes the tab for, and the window carries a
        # title of our own so it can be closed by handle if it is still there.
        $wtTitle = "psmux_keylat_baseline_$PID"
        Start-Process -FilePath $wt.Source -ArgumentList @("-w","new","new-tab","--title",$wtTitle,"--suppressApplicationTitle","--","cmd","/c","pwsh -NoLogo -NoProfile & exit /b 0") | Out-Null
        Start-Sleep -Seconds 6
        $after = @(Get-Process pwsh -EA SilentlyContinue | Select-Object -ExpandProperty Id)
        $new = $after | Where-Object { $before -notcontains $_ }
        if ($new) {
            $wtPid = $new[-1]
            Write-Host "  wt pwsh pid=$wtPid"
            Invoke-KeyLat $wtPid "wt_pwsh" @("--mode","single","--n","$N","--warmup","5","--gap","120")
            Stop-Process -Id $wtPid -Force -EA SilentlyContinue
        } else { Write-Host "  could not identify the wt pwsh process" -ForegroundColor Yellow }
        Close-OwnWtWindow $wtTitle
    } else { Write-Host "  wt.exe not found, skipping" -ForegroundColor Yellow }
    Stop-Bench
}

# ---------------------------------------------------------------- tui
if (& $want "tui") {
    Section "TUI app in the pane (nvim insert mode)"
    $nvim = Get-Command nvim -EA SilentlyContinue
    if ($nvim) {
        $p = Start-PsmuxClient "tui" "nvim -u NONE -n"
        Start-Sleep -Seconds 2
        # 'i' enters insert mode; text then echoes at the cursor
        & $Psmux -L $NS send-keys -t "tui" "i" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        Invoke-KeyLat $p.Id "psmux_nvim_insert" @("--mode","single","--n","$N","--warmup","5","--gap","120","--noerase")
        Stop-Bench

        $b = Start-Bare "nvim" @("-u","NONE","-n")
        Start-Sleep -Seconds 1
        $ins = Join-Path $OutDir "ins.txt"
        Invoke-KeyLat $b.Id "bare_nvim_insert" @("--mode","single","--n","3","--warmup","0","--gap","200","--noerase") | Out-Null
        Invoke-KeyLat $b.Id "bare_nvim_insert" @("--mode","single","--n","$N","--warmup","5","--gap","120","--noerase")
        Stop-Bench
    } else { Write-Host "  nvim not found, skipping" -ForegroundColor Yellow }
}

Stop-Bench

Write-Host ""
Write-Host ("=" * 96) -ForegroundColor Cyan
Write-Host "KEYSTROKE TO SCREEN LATENCY, milliseconds  [tag=$Tag]" -ForegroundColor Cyan
Write-Host ("=" * 96) -ForegroundColor Cyan
$script:Results | Format-Table Scenario, N, @{n='min';e={'{0:F2}' -f $_.Min}}, @{n='median';e={'{0:F2}' -f $_.Median}},
    @{n='mean';e={'{0:F2}' -f $_.Mean}}, @{n='p90';e={'{0:F2}' -f $_.P90}}, @{n='p99';e={'{0:F2}' -f $_.P99}},
    @{n='max';e={'{0:F2}' -f $_.Max}} -AutoSize | Out-String | Write-Host

$csv = Join-Path $OutDir "$Tag`_summary.csv"
$script:Results | Export-Csv -NoTypeInformation -Path $csv
Write-Host "csv: $csv"

$ps = $script:Results | Where-Object { $_.Scenario -eq "psmux_pwsh" }
$bs = $script:Results | Where-Object { $_.Scenario -eq "bare_pwsh" }
if ($ps -and $bs) {
    Write-Host ""
    Write-Host ("PSMUX OVERHEAD OVER BARE CONSOLE: median {0:F2} ms, min {1:F2} ms, p99 {2:F2} ms" -f `
        ($ps.Median - $bs.Median), ($ps.Min - $bs.Min), ($ps.P99 - $bs.P99)) -ForegroundColor Yellow
}
