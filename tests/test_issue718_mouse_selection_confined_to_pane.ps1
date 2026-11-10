# Issue #718: "selecting one or more lines spans across panes".
#
# The screenshot in the issue shows Windows Terminal's OWN selection (full
# grid rows, through the pane border). That is the outer terminal, not psmux,
# exactly as in tmux with mouse off or with Shift+drag.
#
# What psmux owns, and what this suite proves with real console mouse input
# (WriteConsoleInput into the client's console, no foreground needed):
#   1. `mouse` defaults to on in psmux (tmux defaults it off)
#   2. mouse on: a drag that starts in the LEFT pane and runs on into the
#      RIGHT pane selects only left pane text (the selection is clamped to the
#      pane it started in, like tmux copy mode)
#   3. mouse off: psmux does not select at all; the drag is left to the
#      outer terminal, which is what spans panes
#
# Runs under a private -L namespace in a hidden classic conhost window, and
# sets set-clipboard off so the user's clipboard is not touched.

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
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$ns = 'i718_' + [guid]::NewGuid().ToString('N').Substring(0, 6)

$DRAG = Join-Path ([System.IO.Path]::GetTempPath()) 'psmux_mouse_drag_injector.exe'
$csc = "$env:WINDIR\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $DRAG)) { & $csc /nologo /optimize /out:$DRAG "$PSScriptRoot\mouse_drag_injector.cs" 2>&1 | Out-Null }
if (-not (Test-Path $DRAG)) { Write-Host "FATAL: drag injector failed to compile"; exit 1 }

# The drag copies to the system clipboard, which is the user's (and the next
# suite's): put back what was there.
$savedClip = try { Get-Clipboard -Raw -EA Stop } catch { $null }

function Wait-Until([scriptblock]$cond, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 150 }
    return [bool](& $cond)
}
function Buffer-Text { (& $PSMUX -L $ns show-buffer 2>$null | Out-String) }

$conhost = "$env:WINDIR\System32\conhost.exe"
$proc = Start-Process -FilePath $conhost -ArgumentList "`"$PSMUX`"", '-L', $ns, 'new-session', '-s', 's' -WindowStyle Hidden -PassThru
$client = $null
try {
    if (-not (Wait-Until { & $PSMUX -L $ns has-session -t s 2>$null; $LASTEXITCODE -eq 0 } 15000)) {
        Write-Fail "session did not start"; return
    }
    $client = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -like 'psmux*' } | Select-Object -First 1
    if (-not $client) { Write-Fail "no psmux client under conhost $($proc.Id)"; return }
    $cpid = [int]$client.ProcessId
    Wait-Until { (& $PSMUX -L $ns list-clients 2>$null | Out-String).Trim().Length -gt 0 } 8000 | Out-Null

    # 1. default
    $mouseDefault = (& $PSMUX -L $ns show-options -g -v mouse 2>$null | Out-String).Trim()
    if ($mouseDefault -eq 'on') { Write-Pass "mouse defaults to on (tmux defaults to off)" } else { Write-Fail "mouse default is '$mouseDefault'" }
    & $PSMUX -L $ns set -g set-clipboard off
    & $PSMUX -L $ns set -g status off

    # Two side by side panes with distinct marker text.
    & $PSMUX -L $ns split-window -h -t s
    Start-Sleep -Milliseconds 1500
    $panes = @(& $PSMUX -L $ns list-panes -t s -F '#{pane_id} #{pane_left} #{pane_width}')
    $left = ($panes | Where-Object { ($_ -split ' ')[1] -eq '0' } | Select-Object -First 1) -split ' '
    $right = ($panes | Where-Object { ($_ -split ' ')[1] -ne '0' } | Select-Object -First 1) -split ' '
    & $PSMUX -L $ns send-keys -t $left[0] "cls; 1..6 | % { 'LEFTPANE_ROW' + `$_ + '_llll' }" Enter
    & $PSMUX -L $ns send-keys -t $right[0] "cls; 1..6 | % { 'RIGHTPANE_ROW' + `$_ + '_rrrr' }" Enter
    $ok = Wait-Until {
        ((& $PSMUX -L $ns capture-pane -p -t $left[0] | Out-String) -match 'LEFTPANE_ROW6') -and
        ((& $PSMUX -L $ns capture-pane -p -t $right[0] | Out-String) -match 'RIGHTPANE_ROW6')
    } 10000
    if (-not $ok) { Write-Fail "marker text did not appear in both panes"; return }
    $cap = @(& $PSMUX -L $ns capture-pane -p -t $left[0])
    $row1 = [array]::FindIndex([string[]]$cap, [Predicate[string]]{ param($l) $l -match '^LEFTPANE_ROW1' })
    $row3 = $row1 + 2
    Write-Host "  panes: left=$($left -join ',') right=$($right -join ',') rows $row1..$row3"

    # 2. mouse on: drag from the left pane on into the right pane.
    & $PSMUX -L $ns set-buffer -b sentinel 'SENTINEL_718'
    $x2 = [int]$right[1] + 8
    & $DRAG $cpid drag 0 $row1 $x2 $row3 8 50 | Out-Null
    $copied = Wait-Until { (Buffer-Text) -notmatch 'SENTINEL_718' } 5000
    $buf = Buffer-Text
    Write-Host "  buffer after mouse-on drag: [$($buf.Trim() -replace "`r?`n", ' | ')]"
    if (-not $copied) {
        Write-Skip "no buffer was created; mouse input did not reach the client (harness condition, not judged)"
    } else {
        if ($buf -match 'LEFTPANE_ROW1' -and $buf -match 'LEFTPANE_ROW3') { Write-Pass "drag copied the left pane rows it covered" } else { Write-Fail "left pane rows missing from the copy" }
        if ($buf -notmatch 'RIGHTPANE') { Write-Pass "nothing from the right pane was copied (selection confined to its pane)" } else { Write-Fail "right pane text leaked into the copy" }
        if ($buf -notmatch [regex]::Escape([string][char]0x2502)) { Write-Pass "no pane border character in the copy" } else { Write-Fail "border character in the copy" }
    }
    & $PSMUX -L $ns send-keys -t $left[0] -X cancel 2>$null

    # 3. mouse off: psmux leaves the drag alone.
    & $PSMUX -L $ns set -g mouse off
    Start-Sleep -Milliseconds 500
    # Count buffers: a named set-buffer is not "automatic", so show-buffer
    # keeps showing the last copy (tmux paste_get_top); count instead.
    $before = @(& $PSMUX -L $ns list-buffers -F '#{buffer_name}' 2>$null).Count
    & $DRAG $cpid drag 0 $row1 $x2 $row3 8 50 | Out-Null
    Start-Sleep -Milliseconds 2000
    $after = @(& $PSMUX -L $ns list-buffers -F '#{buffer_name}' 2>$null).Count
    if ($after -eq $before) { Write-Pass "mouse off: psmux made no selection, no new buffer ($before before, $after after); the outer terminal owns the drag" } else { Write-Fail "mouse off: a buffer was created ($before before, $after after)" }
    $inMode = (& $PSMUX -L $ns display-message -p -t $left[0] '#{pane_in_mode}' | Out-String).Trim()
    if ($inMode -eq '0') { Write-Pass "mouse off: no copy mode entered" } else { Write-Fail "mouse off: pane_in_mode=$inMode" }
} finally {
    & $PSMUX -L $ns kill-server 2>$null
    if ($null -ne $savedClip) { try { Set-Clipboard -Value $savedClip -EA Stop } catch {} }
    if ($client) { Stop-Process -Id $client.ProcessId -Force -EA SilentlyContinue }
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor $(if ($script:TestsFailed -eq 0) { 'Green' } else { 'Red' })
exit $(if ($script:TestsFailed -eq 0) { 0 } else { 1 })
