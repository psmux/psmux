# probe_keystroke_to_pixels.ps1 - keystroke to PIXELS, psmux against Windows
# Terminal and WezTerm, measured the same way for every host.
#
# WHY A SEPARATE PROBE
# test_perf_vs_terminals' keystroke cells inject into a console input buffer
# and watch a console screen buffer. For a terminal that is UPSTREAM of its
# renderer and of the pseudoconsole pipe, for psmux it is downstream of them,
# so those rows cannot be set side by side (the leaderboard leaves keystroke
# out for that reason). This probe uses tests\pixel_keylat.cs: SendInput into
# the foreground window, exactly like a keyboard, and the first changed pixel
# of the composed desktop as the answer. Every host pays its whole path: input
# stack, terminal, ConPTY, shell echo, (psmux), render, DWM.
#
# THE SAME SHELL EVERYWHERE: pwsh -NoLogo -NoProfile with a two character
# prompt and a STEADY cursor (DECSCUSR 2), so a cursor blink cannot read as a
# fast key and no profile or prompt theme costs one host more than another.
#
# CELLS (interleaved per round, each in a window of its own, closed by PID):
#   conhost_pwsh   pwsh in a classic console window (the floor for "no terminal")
#   wt_pwsh        pwsh in a Windows Terminal window named for this run
#   wezterm_pwsh   pwsh in a WezTerm GUI started for this run
#   psmux_conhost  psmux client in a classic console, pwsh in its pane
#   psmux_in_wt    psmux client in a Windows Terminal window, pwsh in its pane
#
# FOCUS IS THE RISK. SendInput goes to whatever window is in front. The C#
# probe brings its target forward and refuses to type unless the foreground
# window is exactly the target, checked before EVERY key, and stops at the
# first loss. Do not touch the mouse or keyboard while this runs, and run it
# on a QUIET machine: this is a measurement, not a test, and it is not in the
# runner. A cell that reports FOCUS is recorded as such, never as a number.
#
# ISOLATION: own -L namespace and PSMUX_DATA_DIR for the psmux cells; every
# window it opens is closed by PID; WT windows are named psmuxpxl<pid> and
# never one the user opened; the WezTerm GUI is the one this run started.
#
#   pwsh -NoProfile -File tests\probe_keystroke_to_pixels.ps1 [-Rounds 3] [-Keys 30] [-Binary <psmux>]
#
# Writes keystroke-to-pixels-<stamp>.json to the metrics folder.
#
# STATUS (2026-11-10, first run, loaded machine, 10 keys per cell): focus was
# held in every cell and every window closed, so the mechanics work. The
# RESOLUTION does not yet: one GDI strip copy of the composed desktop took
# 5.6 to 21 ms (GRAB_MS, recorded per cell), so a key is only located to within
# one copy, and one reading was implausible (psmux inside WT 15.6 ms against WT
# alone 42.9 ms). Treat the numbers as provisional until this has been run on a
# quiet box, and if GRAB_MS stays above ~2 ms there, replace the GDI copy with
# DXGI desktop duplication, whose frame info carries the QPC time of the
# present itself (DXGI_OUTDUPL_FRAME_INFO.LastPresentTime).
param(
    [string]$Binary = "",
    [int]$Rounds = 3,
    [int]$Keys = 30,
    [int]$GapMs = 150,
    [string[]]$Cells = @("conhost_pwsh", "wt_pwsh", "wezterm_pwsh", "psmux_conhost", "psmux_in_wt"),
    [double]$QuietLoadPct = 25.0
)

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
function Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }

if (-not $Binary) { $Binary = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA SilentlyContinue).Source } }
if (-not $Binary -or -not (Test-Path $Binary)) { Write-Host "psmux binary not found" -ForegroundColor Red; exit 1 }
$Binary = (Resolve-Path $Binary).Path
foreach ($v in @('PSMUX_SESSION', 'PSMUX_SESSION_NAME', 'TMUX', 'TMUX_PANE', 'SHELL')) { Remove-Item "Env:\$v" -EA SilentlyContinue }

$WT = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\wt.exe"
if (-not (Test-Path $WT)) { $c = Get-Command wt.exe -EA SilentlyContinue; $WT = if ($c) { $c.Source } else { $null } }
$WEZ = "C:\Program Files\WezTerm\wezterm.exe"
if (-not (Test-Path $WEZ)) { $c = Get-Command wezterm.exe -EA SilentlyContinue; $WEZ = if ($c) { $c.Source } else { $null } }

$Run = Join-Path $env:TEMP "psmux_pxl_$PID"
New-Item -ItemType Directory -Force -Path $Run | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$Probe = Join-Path $Run "pixel_keylat.exe"
& $csc /nologo /optimize /unsafe "/out:$Probe" (Join-Path $PSScriptRoot "pixel_keylat.cs") | Out-Null
if (-not (Test-Path $Probe)) { Write-Host "could not build pixel_keylat.exe" -ForegroundColor Red; exit 1 }

$NS = "pxl$PID"
$prevDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = Join-Path $Run "data"
New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null

# The shell every cell runs. The title lets the probe find a terminal's window;
# the escape sets a steady block cursor.
function Shell-Cmd { param([string]$Tag) return "[Console]::Write([char]27+'[2 q'); `$host.UI.RawUI.WindowTitle='pxl-$Tag'; function prompt {'> '}" }
function Write-Wrapper {
    param([string]$Tag, [string]$Line)
    $f = Join-Path $Run "w_$Tag.cmd"
    # `exit /b 0` after the shell: when the shell is ended by PID, cmd still
    # exits 0, so a Windows Terminal tab closes instead of staying open with
    # "process exited with code 1".
    Set-Content -Path $f -Encoding ASCII -Value ("@echo off`r`nset PSMUX_DATA_DIR=$env:PSMUX_DATA_DIR`r`n$Line`r`nexit /b 0`r`n")
    return $f
}

# Processes this run started, with their creation time, so a recycled PID is
# never killed. Children are found by parent PID at close time.
$script:Mine = @{}
function Track { param([int]$Id) if ($Id -gt 4) { try { $script:Mine[$Id] = (Get-Process -Id $Id -EA Stop).StartTime } catch {} } }
function Kill-Tracked {
    param([int[]]$Ids)
    foreach ($id in $Ids) {
        if (-not $script:Mine.ContainsKey($id)) { continue }
        $p = Get-Process -Id $id -EA SilentlyContinue
        if ($p -and $p.StartTime -eq $script:Mine[$id]) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    }
}
function Children { param([int]$Id) @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$Id" -EA SilentlyContinue) }

function Run-Probe {
    param([string[]]$TargetArgs, [string]$Tag)
    $out = Join-Path $Run "p_$Tag.txt"
    & $Probe @TargetArgs --n $Keys --gap $GapMs --out $out | Out-Null
    $rc = $LASTEXITCODE
    $lines = if (Test-Path $out) { @(Get-Content $out) } else { @() }
    $lat = @($lines | Where-Object { $_ -match '^LAT \d+ ([\d.]+)' } | ForEach-Object { [double]::Parse(($_ -split ' ')[2], [Globalization.CultureInfo]::InvariantCulture) })
    $grab = ($lines | Where-Object { $_ -match '^GRAB_MS ' } | ForEach-Object { [double]::Parse(($_ -split ' ')[1], [Globalization.CultureInfo]::InvariantCulture) } | Select-Object -First 1)
    $focus = ($lines | Where-Object { $_ -match '^(FOCUS|NOWINDOW|NOCHANGE)' } | Select-Object -First 1)
    return [pscustomobject]@{ rc = $rc; lat = $lat; grab_ms = $grab; problem = $focus }
}

# One cell: open its window, wait for the prompt, probe, close by PID.
function Invoke-Cell {
    param([string]$Cell, [int]$Round)
    $tag = "$Cell-$Round"
    $cmd = Shell-Cmd $tag
    $launched = @(); $res = $null
    switch ($Cell) {
        "conhost_pwsh" {
            $p = Start-Process conhost.exe -ArgumentList @("pwsh", "-NoLogo", "-NoProfile", "-NoExit", "-Command", "`"$cmd`"") -PassThru
            Track $p.Id; $launched += $p.Id
            Start-Sleep -Seconds 3
            $sh = Children $p.Id | Where-Object Name -eq "pwsh.exe" | Select-Object -First 1
            if ($sh) { Track ([int]$sh.ProcessId); $launched += [int]$sh.ProcessId; $res = Run-Probe @("--pid", "$($sh.ProcessId)") $tag }
        }
        "psmux_conhost" {
            $p = Start-Process conhost.exe -ArgumentList @("`"$Binary`"", "-L", $NS, "new-session", "-s", "c$Round", "pwsh", "-NoLogo", "-NoProfile", "-NoExit", "-Command", "`"$cmd`"") -PassThru
            Track $p.Id; $launched += $p.Id
            Start-Sleep -Seconds 4
            $cl = Children $p.Id | Where-Object Name -eq ([IO.Path]::GetFileName($Binary)) | Select-Object -First 1
            if ($cl) { Track ([int]$cl.ProcessId); $launched += [int]$cl.ProcessId; $res = Run-Probe @("--pid", "$($cl.ProcessId)") $tag }
            & $Binary -L $NS kill-session -t "c$Round" 2>&1 | Out-Null
        }
        { $_ -in @("wt_pwsh", "psmux_in_wt") } {
            if (-not $WT) { return $null }
            $line = if ($Cell -eq "wt_pwsh") { "pwsh -NoLogo -NoProfile -NoExit -Command `"$cmd`"" } else { "`"$Binary`" -L $NS new-session -s w$Round pwsh -NoLogo -NoProfile -NoExit -Command `"$cmd`"" }
            $w = Write-Wrapper $tag $line
            $title = "pxl-$tag"
            # --suppressApplicationTitle pins the tab title, so psmux's own title
            # changes cannot hide the window from the probe.
            $p = Start-Process $WT -ArgumentList @("-w", "psmuxpxl$PID-$tag", "nt", "--title", $title, "--suppressApplicationTitle", "cmd", "/c", $w) -PassThru
            Track $p.Id; $launched += $p.Id
            Start-Sleep -Seconds 4
            $res = Run-Probe @("--title", $title) $tag
            if ($Cell -eq "psmux_in_wt") { & $Binary -L $NS kill-session -t "w$Round" 2>&1 | Out-Null }
            # The tab's shell carries this cell's tag on its command line and its
            # cmd wrapper carries the wrapper path: end the shell (or the psmux
            # client) first so the wrapper exits 0 and the tab closes, and only
            # then anything of this cell still running.
            $byTag = { param($needle) @(Get-CimInstance Win32_Process -Filter "Name='cmd.exe' OR Name='pwsh.exe' OR Name='$([IO.Path]::GetFileName($Binary))'" -EA SilentlyContinue |
                Where-Object { $_.CommandLine -and $_.CommandLine.Contains($needle) -and [int]$_.ProcessId -ne $PID }) }
            foreach ($ci in (& $byTag "pxl-$tag")) { if ($ci.Name -ne "cmd.exe") { try { Stop-Process -Id ([int]$ci.ProcessId) -Force -EA SilentlyContinue } catch {} } }
            Start-Sleep -Milliseconds 1000
            foreach ($ci in (& $byTag $w)) { try { Stop-Process -Id ([int]$ci.ProcessId) -Force -EA SilentlyContinue } catch {} }
        }
        "wezterm_pwsh" {
            if (-not $WEZ) { return $null }
            $before = @(Get-Process wezterm-gui -EA SilentlyContinue | ForEach-Object Id)
            $p = Start-Process $WEZ -ArgumentList @("start", "--", "pwsh", "-NoLogo", "-NoProfile", "-NoExit", "-Command", $cmd) -PassThru
            Track $p.Id; $launched += $p.Id
            Start-Sleep -Seconds 4
            foreach ($g in @(Get-Process wezterm-gui -EA SilentlyContinue | Where-Object { $before -notcontains $_.Id })) { Track $g.Id; $launched += $g.Id }
            $res = Run-Probe @("--title", "pxl-$tag") $tag
        }
    }
    # Children of what was launched (shells, console hosts), then the launched.
    foreach ($id in @($launched)) { foreach ($c in (Children $id)) { Track ([int]$c.ProcessId); $launched += [int]$c.ProcessId } }
    Kill-Tracked ($launched | Select-Object -Unique)
    Start-Sleep -Milliseconds 800
    return $res
}

Add-PerfLoadSample "start" | Out-Null
$data = [ordered]@{}
foreach ($c in $Cells) { $data[$c] = [ordered]@{ samples_ms = @(); grab_ms = @(); problems = @() } }
$abort = $false
try {
    for ($r = 0; $r -lt $Rounds -and -not $abort; $r++) {
        Write-Host "  round $r" -ForegroundColor White
        foreach ($c in $Cells) {
            $res = Invoke-Cell $c $r
            if (-not $res) { $data[$c].problems += "round ${r}: not run (host missing or window not found)"; continue }
            $data[$c].samples_ms += $res.lat
            if ($null -ne $res.grab_ms) { $data[$c].grab_ms += $res.grab_ms }
            $st = Get-PerfStats $res.lat 2
            Write-Host ("    {0,-15} n={1,-3} p50={2,7} p90={3,7} max={4,7} ms  grab={5} ms {6}" -f $c, $st.n, $st.p50, $st.p90, $st.max, $res.grab_ms, $res.problem)
            if ($res.problem) {
                $data[$c].problems += "round ${r}: $($res.problem)"
                if ($res.problem -like "FOCUS*") { Warn "focus could not be held; stopping so no key lands in another window"; $abort = $true; break }
            }
        }
        Add-PerfLoadSample "round $r" | Out-Null
    }
} finally {
    & $Binary -L $NS kill-server 2>&1 | Out-Null
    if ($null -eq $prevDataDir) { Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $prevDataDir }
}

Write-Host ""
$summary = [ordered]@{}
foreach ($c in $Cells) {
    $st = Get-PerfStats $data[$c].samples_ms 2
    $data[$c]["stats_ms"] = $st
    $summary[$c] = $st
    Write-Host ("  {0,-15} n={1,-4} p50={2,7} p90={3,7} p99={4,7} max={5,7} ms" -f $c, $st.n, $st.p50, $st.p90, $st.p99, $st.max) -ForegroundColor Green
}
$quiet = Test-PerfMachineQuiet $QuietLoadPct
if (-not $quiet) { Warn ("machine load p50 {0}% is over {1}%: not a quiet box reading" -f (Get-PerfLoadSummary).p50_pct, $QuietLoadPct) }
$jsonPath = Write-PerfMetrics -Suite "probe_keystroke_to_pixels" -Binary $Binary -FileStem "keystroke-to-pixels" -Data ([ordered]@{
    rounds = $Rounds; key_count = $Keys; gap_ms = $GapMs; aborted_on_focus = $abort
    machine_was_quiet = $quiet
    cells = $data
    summary = $summary
})
if ($jsonPath) { Info "metrics: $jsonPath" }
Remove-Item -Recurse -Force $Run -EA SilentlyContinue
exit 0
