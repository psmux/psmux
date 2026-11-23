# #{pane_current_path} asked ONCE (display-message -p, list-panes -F) must
# report where the shell is NOW, never one process table refresh behind.
#
# A `split-window cmd.exe` pane is `pwsh -Command cmd.exe`, so the cwd walk has
# to find cmd.exe as a child in the process table.  The render path table is
# served stale and refreshed behind the caller, so the first query after a
# quiet spell used a table taken before cmd.exe existed, read the wrapper's
# directory, and answered with the folder the user had just left.  It was
# right about 40 ms later, which is too late for a one-shot reply
# (`split-window -c "#{pane_current_path}"`, scripts, test_issue111 test 7).
#
# tmux reads the cwd at query time, so it is current by construction.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$ns = "pcpfresh$PID"
$S = "c"
$Iter = 12
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Cap { ((& $PSMUX -L $ns capture-pane -t $S -p 2>&1) -join '') -replace '\s', '' }
function Wait-Text($pat, [int]$ms = 20000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if ((Cap) -match $pat) { return $true }; Start-Sleep -Milliseconds 50 }
    return $false
}

Write-Host "`n=== pane_current_path query freshness (cmd.exe under the default shell) ===" -ForegroundColor Cyan
$made = @()
try {
    & $PSMUX -L $ns new-session -d -s $S 2>&1 | Out-Null
    & $PSMUX -L $ns has-session -t $S 2>$null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }
    Start-Sleep -Milliseconds 1500

    $stale = 0; $seen = 0; $staleLp = 0
    for ($i = 1; $i -le $Iter; $i++) {
        $d = Join-Path $env:TEMP "psmux_pcpfresh_$(Get-Random)"
        New-Item -ItemType Directory -Force $d | Out-Null
        $made += $d
        $leaf = Split-Path $d -Leaf
        & $PSMUX -L $ns split-window -h -t $S cmd.exe 2>&1 | Out-Null
        if (-not (Wait-Text '>$')) { Write-Host "    iter ${i}: cmd prompt never showed" -ForegroundColor DarkYellow; & $PSMUX -L $ns kill-pane -t $S 2>&1 | Out-Null; continue }
        & $PSMUX -L $ns send-keys -t $S "cd /d `"$d`"" Enter
        if (-not (Wait-Text "$([regex]::Escape($leaf))>$")) { Write-Host "    iter ${i}: prompt for the new folder never showed" -ForegroundColor DarkYellow; & $PSMUX -L $ns kill-pane -t $S 2>&1 | Out-Null; continue }
        $seen++
        # Asked once, right after the prompt for the new folder is on screen.
        $dm = (& $PSMUX -L $ns display-message -t $S -p '#{pane_current_path}' 2>&1 | Out-String).Trim()
        if ($dm -notmatch [regex]::Escape($leaf)) { $stale++; Write-Host "    iter ${i}: display-message said '$dm'" -ForegroundColor DarkYellow }
        $lp = (& $PSMUX -L $ns list-panes -t $S -F '#{pane_active} #{pane_current_path}' 2>&1 | Where-Object { $_ -match '^1 ' } | Out-String).Trim()
        if ($lp -notmatch [regex]::Escape($leaf)) { $staleLp++; Write-Host "    iter ${i}: list-panes said '$lp'" -ForegroundColor DarkYellow }
        & $PSMUX -L $ns kill-pane -t $S 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
    }

    if ($seen -lt [math]::Ceiling($Iter / 2)) { Write-Fail "only $seen of $Iter iterations reached the new prompt; cannot judge" }
    elseif ($stale -eq 0) { Write-Pass "display-message -p reported the new folder in $seen of $seen tries" }
    else { Write-Fail "display-message -p reported the OLD folder in $stale of $seen tries" }
    if ($seen -ge [math]::Ceiling($Iter / 2)) {
        if ($staleLp -eq 0) { Write-Pass "list-panes -F reported the new folder in $seen of $seen tries" }
        else { Write-Fail "list-panes -F reported the OLD folder in $staleLp of $seen tries" }
    }

    # split-window -c '#{pane_current_path}' is the idiom that matters most:
    # the new pane must open where the cmd.exe pane is now.
    $d = Join-Path $env:TEMP "psmux_pcpfresh_split_$(Get-Random)"
    New-Item -ItemType Directory -Force $d | Out-Null
    $made += $d
    $leaf = Split-Path $d -Leaf
    & $PSMUX -L $ns split-window -h -t $S cmd.exe 2>&1 | Out-Null
    [void](Wait-Text '>$')
    & $PSMUX -L $ns send-keys -t $S "cd /d `"$d`"" Enter
    [void](Wait-Text "$([regex]::Escape($leaf))>$")
    & $PSMUX -L $ns split-window -v -t $S -c '#{pane_current_path}' cmd.exe 2>&1 | Out-Null
    # Cap reads the ACTIVE pane, which is the one this split just made.
    $ok = Wait-Text "$([regex]::Escape($leaf))>$" 15000
    if ($ok) { Write-Pass "split-window -c '#{pane_current_path}' opened in the cmd pane's new folder" }
    else { Write-Fail "split-window -c '#{pane_current_path}' did not open in '$leaf': $((Cap))" }

    # TUI: a visible attached client still works with the query route.
    $vis = Start-Process -FilePath $PSMUX -ArgumentList "-L $ns attach -t $S" -PassThru
    Start-Sleep -Seconds 2
    $n = (& $PSMUX -L $ns list-clients 2>&1 | Measure-Object -Line).Lines
    if ($n -ge 1) { Write-Pass "an attached client is live while querying" } else { Write-Fail "attached client did not appear" }
    $dm = (& $PSMUX -L $ns display-message -t $S -p '#{pane_current_path}' 2>&1 | Out-String).Trim()
    if ($dm -match [regex]::Escape($leaf)) { Write-Pass "query with a client attached reports the new folder" } else { Write-Fail "with a client attached it said '$dm'" }
} finally {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    if ($vis -and -not $vis.HasExited) { Stop-Process -Id $vis.Id -Force -EA SilentlyContinue }
    Start-Sleep -Milliseconds 300
    foreach ($d in $made) { Remove-Item $d -Recurse -Force -EA SilentlyContinue }
}

Write-Host "`n  Passed: $script:Pass  Failed: $script:Fail"
exit $script:Fail
