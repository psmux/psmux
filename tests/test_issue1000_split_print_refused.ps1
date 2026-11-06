# Issue #1000: a refused `split-window -P` and the server that died after it.
#
# [1] A refused split with -P exits 1 with the error on stderr, as without -P.
#     It used to print the format for the pane that was active (an existing
#     pane's id) and exit 0, so a caller sent its keys into another program's
#     pane.  A refused split also changes no title (-T) and fires no
#     after-split-window hook (tmux runs an after- hook only on success,
#     cmd-queue.c), and a split that fits still prints the new pane.
# [2] A server started by an attached client gets a hidden console of its own.
#     After the server attached to a pane's console and left it (Ctrl+C, an
#     XTVERSION reply), its std handles still named a console it no longer had,
#     and the error line a refused split printed panicked the server:
#     "failed printing to stderr: The parameter is incorrect. (os error 87)".
#
# Run: pwsh -NoProfile -File tests\test_issue1000_split_print_refused.ps1
# PSMUX_TEST_BIN overrides the binary under test.

$ErrorActionPreference = 'Continue'
$env:SHELL = $null
$ExePath = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
$DataDir = Join-Path ([IO.Path]::GetTempPath()) "psmux-i1000-$PID"
New-Item -ItemType Directory -Force $DataDir | Out-Null
$env:PSMUX_DATA_DIR = $DataDir
$script:Passed = 0
$script:Failed = 0
$script:NS = "i1000a$PID"
$script:Window = $null

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { $script:Passed++; Write-Output "  [PASS] $Name" } else { $script:Failed++; Write-Output "  [FAIL] $Name $Detail" }
}
function P { & $ExePath -L $script:NS @args }
function Panes { @(P list-panes -t t -F '#{pane_id}') }
# Runs a split with -P, returning exit code, stdout and stderr apart.
function Split-Print {
    param([string[]]$Extra = @())
    $errFile = Join-Path $DataDir 'stderr.txt'
    $out = & $ExePath -L $script:NS split-window -t t -v -d -P -F '#{pane_id}' @Extra 2> $errFile
    [pscustomobject]@{ Rc = $LASTEXITCODE; Out = (($out | Out-String).Trim()); Err = ((Get-Content $errFile -Raw) + '').Trim() }
}
# Splits until the window refuses; returns the refused result.
function Fill-Window {
    for ($i = 0; $i -lt 12; $i++) {
        $r = Split-Print
        if ($r.Out -notmatch '^%\d+$') { return $r }
    }
    $null
}

try {
    Write-Output "[1] refused split-window -P, detached session"
    P new-session -d -s t -x 100 -y 16 | Out-Null
    Start-Sleep -Milliseconds 1500
    $r = Split-Print
    Check ($r.Rc -eq 0 -and $r.Out -match '^%\d+$' -and (Panes) -contains $r.Out) "a split that fits prints the new pane, which exists" "(rc=$($r.Rc) out=[$($r.Out)])"

    $refused = Fill-Window
    Check ($null -ne $refused) "the window fills until a split is refused"
    $before = Panes
    $r = Split-Print
    Check ($r.Rc -ne 0) "a refused split -P exits non-zero" "(rc=$($r.Rc))"
    Check ($r.Out -eq '') "a refused split -P prints no pane id on stdout" "(out=[$($r.Out)])"
    Check ($r.Err -match 'split-window: pane too small') "a refused split -P puts the error on stderr" "(err=[$($r.Err)])"
    $after = Panes
    Check ($before.Count -eq $after.Count) "a refused split makes no pane" "($($before.Count) -> $($after.Count))"

    # The same refusal without -P, for comparison.
    $out = & $ExePath -L $script:NS split-window -t t -v -d 2>&1
    $plainRc = $LASTEXITCODE
    Check ($plainRc -eq $r.Rc -and ($out -join '') -match 'pane too small') "without -P the refusal exits the same way" "(rc=$plainRc out=[$($out -join ' ')])"

    # -T on a refused split leaves the active pane's title alone.
    P select-pane -t t -T KEEPTITLE | Out-Null
    $active = ((P display-message -p -t t '#{pane_id}') -join '').Trim()
    $r = Split-Print -Extra @('-T', 'STOLEN')
    $title = ((P display-message -p -t $active '#{pane_title}') -join '').Trim()
    Check ($title -eq 'KEEPTITLE') "a refused split -P -T leaves the existing pane's title" "(title=[$title])"
    $null = & $ExePath -L $script:NS split-window -t t -v -d -T STOLEN2 2>&1
    $title = ((P display-message -p -t $active '#{pane_title}') -join '').Trim()
    Check ($title -eq 'KEEPTITLE') "a refused split -T (no -P) leaves the existing pane's title" "(title=[$title])"

    # after-split-window fires only for a split that happened.
    P set-hook -g after-split-window 'set -g @i1000_after yes' | Out-Null
    $null = Split-Print
    $null = & $ExePath -L $script:NS split-window -t t -v -d 2>&1
    Start-Sleep -Milliseconds 400
    $hooked = ((P show-options -gqv '@i1000_after') -join '').Trim()
    Check ($hooked -eq '') "a refused split fires no after-split-window hook" "(@i1000_after=[$hooked])"
    P kill-pane -t ((Panes)[-1]) | Out-Null
    P select-layout -t t even-vertical | Out-Null
    Start-Sleep -Milliseconds 300
    $r = Split-Print
    Start-Sleep -Milliseconds 400
    $hooked = ((P show-options -gqv '@i1000_after') -join '').Trim()
    Check ($r.Out -match '^%\d+$' -and $hooked -eq 'yes') "a split that happens fires after-split-window" "(out=[$($r.Out)] @i1000_after=[$hooked])"
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500

    Write-Output "[2] a server started by an attached client survives refused splits after a console attach"
    $script:NS = "i1000b$PID"
    $env:PSMUX_NO_WARM = '1'
    $script:Window = Start-Process -FilePath $ExePath -ArgumentList @('-L', $script:NS, 'new-session', '-s', 't', 'pwsh -NoProfile -NoLogo') -PassThru
    Remove-Item Env:PSMUX_NO_WARM -ErrorAction SilentlyContinue
    $up = $false
    for ($i = 0; $i -lt 60 -and -not $up; $i++) { Start-Sleep -Milliseconds 250; P has-session -t t 2>$null | Out-Null; $up = $LASTEXITCODE -eq 0 }
    Check $up "the attached session is up"
    Start-Sleep -Seconds 3
    $spid = [int]((P display-message -p -t t '#{pid}') -join '').Trim()
    # Both make the server attach to the pane's console and leave it again.
    P send-keys -t t "[Console]::Write([char]27 + '[>q'); 'ASKED'" Enter | Out-Null
    Start-Sleep -Seconds 1
    P send-keys -t t C-c | Out-Null
    Start-Sleep -Seconds 1
    $refusals = 0
    for ($i = 0; $i -lt 10 -and $refusals -lt 3; $i++) {
        $r = Split-Print
        if ($r.Out -notmatch '^%\d+$') { $refusals++ }
        if (-not (Get-Process -Id $spid -ErrorAction SilentlyContinue)) { break }
    }
    Start-Sleep -Seconds 1
    Check ($refusals -ge 3) "three splits were refused" "(refusals=$refusals)"
    Check ([bool](Get-Process -Id $spid -ErrorAction SilentlyContinue)) "the server is still running after the refused splits"
    P has-session -t t 2>$null | Out-Null
    Check ($LASTEXITCODE -eq 0) "the session still answers"
    $crash = @(Get-ChildItem $DataDir -Recurse -Filter 'crash*.log' -ErrorAction SilentlyContinue)
    $crashText = ($crash | ForEach-Object { Get-Content $_.FullName -Raw }) -join ''
    Check ($crashText -notmatch 'failed printing') "no 'failed printing to stderr' panic in crash.log" "($($crashText -split "`n" | Select-Object -First 3))"
}
finally {
    P kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 500
    if ($script:Window -and (Get-Process -Id $script:Window.Id -ErrorAction SilentlyContinue)) { Stop-Process -Id $script:Window.Id -Force }
    Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
}

Write-Output "=== Results: $script:Passed passed, $script:Failed failed ==="
exit $script:Failed
