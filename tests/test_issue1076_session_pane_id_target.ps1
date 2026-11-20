# Issue #1076: session qualified pane ids (`sess:%N`).
#
# tmux 3.8 (measured on a real tmux 3.8 and 3.6, and read in cmd-find.c)
# treats the text after the colon as a WINDOW when there is no period, so
# `sess:%N` is rejected with "can't find window: %N" by every command that
# takes a pane target. The tmux forms that name a pane inside a session are
# `sess:.%N` and `sess:@W.%N`. This suite pins psmux to that behaviour:
#   * `sess:.%N` and `sess:@W.%N` resolve to pane %N of that session,
#   * the same forms with a pane id this session lacks are "can't find pane",
#   * `sess:%N` stays an error naming the window, like tmux.
#
# Uses its own -L namespace and PSMUX_DATA_DIR, honours PSMUX_TEST_BIN.

$ErrorActionPreference = 'Continue'
$env:PSMUX_RUN_NOPAUSE = '1'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
$NS = 'r16t1076' + (Get-Random -Maximum 99999)
$dataDir = Join-Path ([IO.Path]::GetTempPath()) ("psmux_" + $NS)
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$oldData = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $dataDir

$pass = 0; $fail = 0
function Ok($m) { $script:pass++; Write-Host "  [PASS] $m" -ForegroundColor Green }
function Bad($m) { $script:fail++; Write-Host "  [FAIL] $m" -ForegroundColor Red }

function Run([string[]]$a) {
    $out = & $PSMUX -L $NS @a 2>&1
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = (($out | ForEach-Object { "$_" }) -join "`n").Trim() }
}

function Wait-Session($name) {
    for ($i = 0; $i -lt 100; $i++) {
        & $PSMUX -L $NS has-session -t $name 2>$null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

try {
    Write-Host "Issue #1076 session qualified pane id targets ($PSMUX, -L $NS)"
    Run @('new-session', '-d', '-s', 'alpha', '-x', '100', '-y', '30') | Out-Null
    if (-not (Wait-Session 'alpha')) { throw 'alpha did not start' }
    Run @('new-session', '-d', '-s', 'beta', '-x', '100', '-y', '30') | Out-Null
    if (-not (Wait-Session 'beta')) { throw 'beta did not start' }
    Run @('split-window', '-d', '-t', 'alpha') | Out-Null
    Start-Sleep -Milliseconds 500

    $rows = (Run @('list-panes', '-a', '-F', '#{session_name}|#{window_id}|#{pane_id}|#{pane_active}')).Text -split "`n"
    $alpha = $rows | Where-Object { $_ -like 'alpha|*' } | ForEach-Object { $f = $_ -split '\|'; [pscustomobject]@{ W = $f[1]; P = $f[2]; A = $f[3] } }
    $beta = $rows | Where-Object { $_ -like 'beta|*' } | ForEach-Object { $f = $_ -split '\|'; [pscustomobject]@{ W = $f[1]; P = $f[2]; A = $f[3] } }
    $inactive = ($alpha | Where-Object { $_.A -eq '0' } | Select-Object -First 1)
    $betaPane = ($beta | Select-Object -First 1)
    if (-not $inactive -or -not $betaPane) { throw "unexpected pane layout: $($rows -join ' / ')" }
    $aw = $inactive.W; $ap = $inactive.P; $bp = $betaPane.P
    Write-Host "  alpha inactive pane $aw.$ap, beta pane $bp"

    # Marker in the inactive alpha pane so capture-pane proves which pane it read.
    $marker = 'MARK1076_' + (Get-Random -Maximum 999999)
    Run @('send-keys', '-t', "alpha:$aw.$ap", "echo $marker", 'Enter') | Out-Null
    $seen = $false
    for ($i = 0; $i -lt 50 -and -not $seen; $i++) {
        Start-Sleep -Milliseconds 200
        $c = Run @('capture-pane', '-p', '-t', "alpha:$aw.$ap")
        if ($c.Text -match "(?m)^$marker") { $seen = $true }
    }
    if ($seen) { Ok "marker reached alpha:$aw.$ap" } else { Bad "marker never appeared in alpha:$aw.$ap" }

    Write-Host "`n[1] tmux forms that name a pane inside a session"
    foreach ($t in @("alpha:.$ap", "alpha:$aw.$ap")) {
        $r = Run @('display-message', '-t', $t, '-p', '#{session_name}|#{pane_id}')
        if ($r.Code -eq 0 -and $r.Text -eq "alpha|$ap") { Ok "display-message -t $t -> $($r.Text)" }
        else { Bad "display-message -t $t -> rc=$($r.Code) '$($r.Text)' (want alpha|$ap)" }
        $r = Run @('capture-pane', '-p', '-t', $t)
        if ($r.Code -eq 0 -and $r.Text -match $marker) { Ok "capture-pane -t $t read the marked pane" }
        else { Bad "capture-pane -t $t rc=$($r.Code) did not show the marker" }
    }

    # psmux numbers panes per session server, so beta's id may also exist in
    # alpha. Use an id alpha does not have to check the not found path.
    $alphaIds = @($alpha | ForEach-Object { $_.P })
    $missing = '%' + (1000 + (Get-Random -Maximum 8000))
    while ($alphaIds -contains $missing) { $missing = '%' + (1000 + (Get-Random -Maximum 8000)) }
    Write-Host "`n[2] a pane id that is not in this session ($missing)"
    foreach ($t in @("alpha:.$missing", "alpha:$aw.$missing")) {
        foreach ($cmd in @(@('capture-pane', '-p'), @('send-keys'), @('select-pane'))) {
            $args2 = $cmd + @('-t', $t)
            if ($cmd[0] -eq 'send-keys') { $args2 += 'x' }
            $r = Run $args2
            if ($r.Code -ne 0 -and $r.Text -match "can't find pane") { Ok "$($cmd[0]) -t $t -> $($r.Text)" }
            else { Bad "$($cmd[0]) -t $t -> rc=$($r.Code) '$($r.Text)' (want can't find pane)" }
        }
    }

    Write-Host "`n[3] sess:%N is a window lookup, as in tmux 3.8"
    foreach ($cmd in @(@('capture-pane', '-p'), @('send-keys'), @('select-pane'), @('kill-pane'), @('resize-pane', '-U', '1'))) {
        $args2 = $cmd + @('-t', "alpha:$ap")
        if ($cmd[0] -eq 'send-keys') { $args2 += 'x' }
        $r = Run $args2
        if ($r.Code -ne 0 -and $r.Text -match "can't find window: $([regex]::Escape($ap))") { Ok "$($cmd[0]) -t alpha:$ap -> $($r.Text)" }
        else { Bad "$($cmd[0]) -t alpha:$ap -> rc=$($r.Code) '$($r.Text)' (want can't find window: $ap)" }
    }

    $after = (Run @('list-panes', '-a', '-F', '#{pane_id}')).Text -split "`n"
    if ($after -contains $ap -and $after -contains $bp) { Ok 'no pane was killed by the rejected targets' }
    else { Bad "panes changed: $($after -join ',')" }
}
catch { Bad "exception: $_" }
finally {
    & $PSMUX -L $NS kill-server 2>$null | Out-Null
    $env:PSMUX_DATA_DIR = $oldData
    Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
}

Write-Host "`nResult: $pass passed, $fail failed"
if ($fail -gt 0) { exit 1 } else { exit 0 }
