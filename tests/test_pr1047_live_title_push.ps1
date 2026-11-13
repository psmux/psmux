# PR #1047: a program that only changes its title (a spinner in the title, no
# other output) must reach the attached client without anyone querying the
# server. The parser holds the new OSC title at once, but master copied it into
# pane.title only on a state query (DumpState, list-*, display-message), so the
# frames the server pushed by itself expanded #{pane_title} from an old value:
# the status line, the pane border and the host title lagged or froze.
#
# tmux updates the pane title as the OSC is parsed (input.c input_exit_osc ->
# screen_set_title, then server_redraw_window_borders / server_status_window),
# so every redraw shows the newest title.
#
# A real client in its own console; its screen is sampled with
# tests\console_screen_read.cs. NOTHING queries the server while sampling:
# a query would itself refresh the title and hide the bug.
#
# Private PSMUX_DATA_DIR and config, a -L namespace per run; cleanup is
# `-L <ns> kill-server` and the client PID this suite started.
# PSMUX_TEST_BIN (or PSMUX_EXE) picks the binary.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_BIN' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ttl_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"
$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$READ = Join-Path $TMP "screenread.exe"
if ($csc) { & $csc /nologo /optimize "/out:$READ" (Join-Path $PSScriptRoot "console_screen_read.cs") 2>&1 | Out-Null }
if (-not (Test-Path $READ)) { Write-Error "could not build tests\console_screen_read.cs"; exit 1 }
$POWERSHELL = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $POWERSHELL) { $POWERSHELL = (Get-Command powershell).Source }

# 20 titles, 300 ms apart, and nothing else written.
$spinner = Join-Path $TMP "spinner.ps1"
@'
Start-Sleep -Seconds 2
for ($i = 0; $i -lt 20; $i++) {
    [Console]::Write("`e]2;TF_$($i.ToString('00'))`a")
    [Console]::Out.Flush()
    Start-Sleep -Milliseconds 300
}
Start-Sleep -Seconds 60
'@ | Set-Content $spinner -Encoding UTF8

$script:client = $null
$script:ns = ""
function Start-Run($mode) {
    $script:ns = "ttl_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $conf = Join-Path $TMP "$($script:ns).conf"
    @("set -g default-shell '$POWERSHELL -NoLogo -NoProfile'",
      "set -g allow-set-title on",
      "set -g automatic-rename off",
      "set -g status-right 'R[#{pane_title}]'",
      "set -g pane-border-status top",
      "set -g pane-border-format 'B[#{pane_title}]'") -join "`n" | Set-Content $conf -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $conf
    $env:PSMUX_SERVER_MODE = $mode
    & $PSMUX -L $script:ns new-session -d -s A -x 100 -y 25 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $script:ns has-session -t A 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 800
    $script:client = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $script:ns, 'attach', '-t', 'A') -PassThru
    Start-Sleep -Seconds 4
}
function Stop-Run {
    if ($script:ns) { & $PSMUX -L $script:ns kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    if ($script:client -and -not $script:client.HasExited) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
    $script:client = $null; $script:ns = ""
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
}

foreach ($mode in 'per-session', 'shared') {
    Write-Host "`n=== PR #1047 live pane title in pushed frames ($mode server) ===" -ForegroundColor Cyan
    Write-Host "  Binary: $PSMUX"
    try {
        Start-Run $mode
        & $PSMUX -L $script:ns send-keys -t A -l "& '$spinner'" 2>&1 | Out-Null
        & $PSMUX -L $script:ns send-keys -t A Enter 2>&1 | Out-Null
        # From here on only the client's own screen is read.
        $status = @{}; $border = @{}
        $f = Join-Path $TMP "screen.txt"
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 11) {
            & $READ $script:client.Id $f | Out-Null
            $text = (Get-Content $f -Encoding UTF8 | Where-Object { $_.StartsWith('T|') }) -join "`n"
            foreach ($m in [regex]::Matches($text, 'R\[TF_(\d\d)\]')) { $status[$m.Groups[1].Value] = 1 }
            foreach ($m in [regex]::Matches($text, 'B\[TF_(\d\d)\]')) { $border[$m.Groups[1].Value] = 1 }
            Start-Sleep -Milliseconds 60
        }
        $s = @($status.Keys | Sort-Object); $b = @($border.Keys | Sort-Object)
        Write-Host "  status-right saw $($s.Count): $($s -join ' ')"
        Write-Host "  pane border saw $($b.Count): $($b -join ' ')"
        if ($s.Count -ge 15 -and $s -contains '19') { Write-Pass "the status line follows the title live ($($s.Count) of 20)" } else { Write-Fail "the status line lagged: $($s.Count) of 20 titles seen" }
        if ($b.Count -ge 15 -and $b -contains '19') { Write-Pass "the pane border follows the title live ($($b.Count) of 20)" } else { Write-Fail "the pane border lagged: $($b.Count) of 20 titles seen" }
        # Only now ask the server: the title itself was set.
        $t = (& $PSMUX -L $script:ns display-message -p -t A '#{pane_title}' 2>&1 | Out-String).Trim()
        if ($t -eq 'TF_19') { Write-Pass "the pane title ends at TF_19" } else { Write-Fail "the pane title should end at TF_19, got [$t]" }
    }
    catch {
        Write-Fail "exception: $_"
    }
    finally {
        Stop-Run
    }
}
Remove-Item -Recurse -Force $TMP -EA SilentlyContinue

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
