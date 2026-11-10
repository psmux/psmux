# Issue #712 audit: byte index slices of text that panicked the SERVER on
# non ASCII input, taking every pane in the session down with it.
#
# Each case starts a fresh server under a private -L namespace, sends one
# command that used to panic, then asserts the server process is still
# alive and answers has-session. The copy mode search case also asserts the
# cursor lands on the hit's DISPLAY column (a CJK character is two cells).
#
# Reproduced on master before the fix (crash.log):
#   copy_mode.rs:1222 start byte index 2 is not a char boundary; it is inside '日'
#   format.rs:657/689/703 start byte index 1 is not a char boundary
#   util.rs:733 end byte index 3 is not a char boundary; it is inside 'é'
#   pane.rs:2142/2185 byte range starts at 1 but ends at 0

$ErrorActionPreference = 'Continue'
# psmux writes UTF-8, and PowerShell decodes a native command's output with
# [Console]::OutputEncoding, the console code page (OEM 437 in the runner's
# window), so the non-ASCII text this suite compares must be read as UTF-8.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = (Get-Command psmux -EA Stop).Source
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i712s_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$nsBase = 'i712s_' + [guid]::NewGuid().ToString('N').Substring(0, 6)
$fix = Join-Path $work 'lines.txt'
[System.IO.File]::WriteAllText($fix, "x日本y日本z`nécoleécoleécole`n", $utf8)

function Wait-Until([scriptblock]$cond, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return [bool](& $cond)
}

$i = 0
function Run-Case([string]$name, [scriptblock]$action, [scriptblock]$check) {
    $script:i++
    $ns = "${nsBase}_$($script:i)"
    Write-Host "`n[Case $($script:i)] $name" -ForegroundColor Yellow
    & $PSMUX -L $ns new-session -d -s s -x 100 -y 12
    if (-not (Wait-Until { & $PSMUX -L $ns has-session -t s 2>$null; $LASTEXITCODE -eq 0 } 8000)) {
        Write-Fail "session did not start"; & $PSMUX -L $ns kill-server 2>$null; return
    }
    $srv = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match " -L $ns( |$)" -and $_.CommandLine -match ' server ' -and $_.CommandLine -notmatch '__warm__' } |
        Select-Object -First 1
    try {
        $out = & $action $ns
        Start-Sleep -Milliseconds 700
        $alive = $srv -and [bool](Get-Process -Id $srv.ProcessId -EA SilentlyContinue)
        & $PSMUX -L $ns has-session -t s 2>$null
        if ($alive -and $LASTEXITCODE -eq 0) { Write-Pass "server alive and answering after: $name" }
        else { Write-Fail "server DIED after: $name (pid $($srv.ProcessId))" }
        if ($check) { & $check $ns $out }
    } finally {
        & $PSMUX -L $ns kill-server 2>$null
    }
}

function Show-Fixture($ns) {
    & $PSMUX -L $ns send-keys -t s "cls; Get-Content -Encoding utf8 '$fix'" Enter
    Wait-Until { (& $PSMUX -L $ns capture-pane -p -t s | Out-String).Contains('école') } 10000 | Out-Null
}

Run-Case 'copy mode search-forward 日本 (3 byte first char)' {
    param($ns)
    Show-Fixture $ns
    & $PSMUX -L $ns copy-mode -t s
    & $PSMUX -L $ns send-keys -t s -X history-top
    & $PSMUX -L $ns send-keys -t s -X start-of-line
    $cols = @()
    foreach ($k in 1..2) {
        & $PSMUX -L $ns send-keys -t s -X search-forward '日本'
        $cols += (& $PSMUX -L $ns display-message -p -t s '#{copy_cursor_x}' | Out-String).Trim()
    }
    return $cols
} {
    param($ns, $cols)
    # "x日本y日本z": x=0, 日=1..2, 本=3..4, y=5, 日=6. tmux puts the cursor on cell 1 then cell 6.
    if ($cols[0] -eq '1' -and $cols[1] -eq '6') { Write-Pass "search hits land on display columns 1 and 6" }
    else { Write-Fail "search hit columns were '$($cols -join ",")', expected '1,6'" }
}

Run-Case 'copy mode search-backward école (2 byte first char)' {
    param($ns)
    Show-Fixture $ns
    & $PSMUX -L $ns copy-mode -t s
    & $PSMUX -L $ns send-keys -t s -X search-backward 'école'
} $null

Run-Case "format s modifier with a non ASCII wrapper '#{s§s§X§:session_name}'" {
    param($ns) & $PSMUX -L $ns display-message -p '#{s§s§X§:session_name}' 2>&1
} $null

Run-Case "format e modifier with a non ASCII wrapper '#{e§+§:1,2}'" {
    param($ns) & $PSMUX -L $ns display-message -p '#{e§+§:1,2}' 2>&1
} $null

Run-Case "format chain with a non ASCII segment '#{t;é:session_name}'" {
    param($ns) & $PSMUX -L $ns display-message -p '#{t;é:session_name}' 2>&1
} $null

Run-Case "pipe-pane 'caé foo' (byte 3 inside é)" {
    param($ns) & $PSMUX -L $ns pipe-pane -t s 'caé foo' 2>&1
} $null

Run-Case "new-window with a lone quote: /bin/bash -c '" {
    param($ns) & $PSMUX -L $ns new-window "/bin/bash -c '" 2>&1
} $null

Run-Case "new-window with export X=`" (lone quote value)" {
    param($ns) & $PSMUX -L $ns new-window "/bin/bash -c 'export X=`"'" 2>&1
} $null

Remove-Item -Recurse -Force $work -EA SilentlyContinue
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor $(if ($script:TestsFailed -eq 0) { 'Green' } else { 'Red' })
exit $(if ($script:TestsFailed -eq 0) { 0 } else { 1 })
