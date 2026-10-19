# Issue #811: in control mode, kill-window on a window that is not the
# current one announced `%window-close` for the CURRENT window. tmux names
# the window that closed (control-notify.c control_notify_window_unlinked:
# `%window-close @N` for a window of the client's session, otherwise
# `%unlinked-window-close @N`). Also, new-window making the new window
# current announces `%session-window-changed $S @N` in tmux
# (control_notify_session_window_changed).
#
# Drives a real `psmux -L <ns> -CC new-session -A -s demo` over pipes.
# Private -L namespace, cleaned up with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i811_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'i811_' + [guid]::NewGuid().ToString('N').Substring(0, 8)

function Read-Shared([string]$path) {
    try {
        $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
        $ms = [System.IO.MemoryStream]::new(); $fs.CopyTo($ms); $fs.Close()
        return , $ms.ToArray()
    } catch { return , [byte[]]@() }
}
function Get-Lines([string]$path) { return @($utf8.GetString((Read-Shared $path)) -split "`n" | ForEach-Object { $_.TrimEnd("`r") }) }
function Wait-Until([scriptblock]$cond, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 20 }
    return [bool](& $cond)
}
function Get-Blocks([string]$path) {
    $blocks = @(); $cur = $null
    foreach ($l in (Get-Lines $path)) {
        if ($l -match '^%begin ') { $cur = @{ Body = @(); End = $null }; continue }
        if ($null -ne $cur) {
            if ($l -match '^%(end|error) ') { $cur.End = $Matches[1]; $blocks += [pscustomobject]$cur; $cur = $null; continue }
            if ($l -notmatch '^%[a-z][a-z-]*( |$)') { $cur.Body += $l }
        }
    }
    return , $blocks
}
# Notification lines (outside %begin blocks) from line index $from on.
function Get-Notes([int]$from) {
    $lines = Get-Lines $out; $notes = @(); $in = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $l = $lines[$i]
        if ($l -match '^%begin ') { $in = $true; continue }
        if ($in) { if ($l -match '^%(end|error) ') { $in = $false }; continue }
        if ($i -ge $from -and $l -match '^%') { $notes += $l }
    }
    return , $notes
}

$psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $ns -CC new-session -A -s demo")
$psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
$psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
$out = Join-Path $work 'cc.out'; $err = Join-Path $work 'cc.err'
$p = [System.Diagnostics.Process]::Start($psi)
$fo = [System.IO.FileStream]::new($out, 'Create', 'Write', 'ReadWrite', 1)
$fe = [System.IO.FileStream]::new($err, 'Create', 'Write', 'ReadWrite', 1)
$t1 = $p.StandardOutput.BaseStream.CopyToAsync($fo, 1)
$t2 = $p.StandardError.BaseStream.CopyToAsync($fe, 1)

function Invoke-CC([string]$line, [int]$expect = 1, [int]$waitMs = 7000) {
    $before = (Get-Blocks $out).Count
    $p.StandardInput.Write("$line`n"); $p.StandardInput.Flush()
    Wait-Until { (Get-Blocks $out).Count -ge $before + $expect } $waitMs | Out-Null
    $all = Get-Blocks $out
    $new = @(); for ($i = $before; $i -lt $all.Count; $i++) { $new += $all[$i] }
    return , $new
}
# Run a command and collect the notifications that follow it.
function Invoke-Notes([string]$line, [scriptblock]$until, [int]$waitMs = 3000) {
    $from = (Get-Lines $out).Count - 1
    $r = Invoke-CC $line
    Wait-Until { & $until (Get-Notes $from) } $waitMs | Out-Null
    Start-Sleep -Milliseconds 400
    return [pscustomobject]@{ Blocks = $r; Notes = (Get-Notes $from) }
}

try {
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 10000)) {
        Write-Fail "-CC session did not start"; throw 'no session'
    }
    $ids = (Invoke-CC "display-message -p '#{session_id} #{window_id}'")[0].Body[0].Trim() -split ' '
    $sid = $ids[0]; $w1 = $ids[1]
    Write-Host "session $sid, first window $w1"

    Write-Host "`n[1] new-window announces the add and the current window change" -ForegroundColor Yellow
    $r = Invoke-Notes "new-window" { param($n) ($n | Where-Object { $_ -match '^%window-add ' }).Count -gt 0 }
    $add = $r.Notes | Where-Object { $_ -match '^%window-add (@\d+)$' } | Select-Object -First 1
    $w2 = if ($add -match '(@\d+)') { $Matches[1] } else { $null }
    if ($w2) { Write-Pass "new-window -> %window-add $w2" } else { Write-Fail "no %window-add: $($r.Notes -join ' / ')" }
    if ($r.Notes -contains "%session-window-changed $sid $w2") { Write-Pass "new-window -> %session-window-changed $sid $w2" } else { Write-Fail "new-window: no '%session-window-changed $sid $w2' in: $($r.Notes -join ' / ')" }

    $r = Invoke-Notes "rename-window -t '$w2' 'renamed'" { param($n) ($n | Where-Object { $_ -match '^%window-renamed ' }).Count -gt 0 }
    if ($r.Notes -contains "%window-renamed $w2 renamed") { Write-Pass "rename-window -> %window-renamed $w2 renamed" } else { Write-Fail "rename-window: $($r.Notes -join ' / ')" }

    $r = Invoke-Notes "select-window -t '$w1'" { param($n) ($n | Where-Object { $_ -match '^%session-window-changed ' }).Count -gt 0 }
    if ($r.Notes -contains "%session-window-changed $sid $w1") { Write-Pass "select-window -> %session-window-changed $sid $w1" } else { Write-Fail "select-window: $($r.Notes -join ' / ')" }

    Write-Host "`n[2] kill-window on the other window names that window" -ForegroundColor Yellow
    $r = Invoke-Notes "kill-window -t '$w2'" { param($n) ($n | Where-Object { $_ -match 'window-close ' }).Count -gt 0 }
    $close = @($r.Notes | Where-Object { $_ -match 'window-close ' })
    if ($close.Count -eq 1 -and $close[0] -eq "%window-close $w2") { Write-Pass "kill-window -t $w2 -> '$($close[0])'" } else { Write-Fail "kill-window -t $w2 -> close notes: '$($close -join ' / ')' (all: $($r.Notes -join ' / '))" }
    if (-not ($r.Notes | Where-Object { $_ -match "window-close $w1$" })) { Write-Pass "no close announced for the surviving $w1" } else { Write-Fail "close announced for $w1" }
    $lw = (Invoke-CC "list-windows -F '#{window_id}'")[0].Body | ForEach-Object { $_.Trim() } | Where-Object { $_ }
    if (($lw -join ',') -eq $w1) { Write-Pass "list-windows -> $w1 only" } else { Write-Fail "list-windows -> $($lw -join ',')" }

    Write-Host "`n[3] kill-window on the current window still names it" -ForegroundColor Yellow
    Invoke-CC "new-window" | Out-Null
    Start-Sleep -Milliseconds 500
    $w3 = (Invoke-CC "display-message -p '#{window_id}'")[0].Body[0].Trim()
    $r = Invoke-Notes "kill-window -t '$w3'" { param($n) ($n | Where-Object { $_ -match 'window-close ' }).Count -gt 0 }
    $close = @($r.Notes | Where-Object { $_ -match 'window-close ' })
    if ($close.Count -eq 1 -and $close[0] -eq "%window-close $w3") { Write-Pass "kill-window current $w3 -> '$($close[0])'" } else { Write-Fail "kill-window current $w3 -> '$($close -join ' / ')'" }
    if ($r.Notes -contains "%session-window-changed $sid $w1") { Write-Pass "kill of the current window -> %session-window-changed $sid $w1" } else { Write-Fail "kill current: no session-window-changed to $w1 in $($r.Notes -join ' / ')" }

    Write-Host "`n[4] kill-window from another client (CLI) on the non current window" -ForegroundColor Yellow
    Invoke-CC "new-window" | Out-Null
    Start-Sleep -Milliseconds 500
    $w4 = (Invoke-CC "display-message -p '#{window_id}'")[0].Body[0].Trim()
    Invoke-CC "select-window -t '$w1'" | Out-Null
    Start-Sleep -Milliseconds 300
    $from = (Get-Lines $out).Count - 1
    & $PSMUX -L $ns kill-window -t $w4
    Wait-Until { ((Get-Notes $from) | Where-Object { $_ -match 'window-close ' }).Count -gt 0 } 3000 | Out-Null
    Start-Sleep -Milliseconds 400
    $close = @((Get-Notes $from) | Where-Object { $_ -match 'window-close ' })
    if ($close.Count -eq 1 -and $close[0] -eq "%window-close $w4") { Write-Pass "CLI kill-window -t $w4 -> '$($close[0])'" } else { Write-Fail "CLI kill-window -t $w4 -> '$($close -join ' / ')'" }

    Write-Host "`n[5] new-window -d adds the new window and keeps the current one" -ForegroundColor Yellow
    $from = (Get-Lines $out).Count - 1
    $w5 = (& $PSMUX -L $ns new-window -d -P -F '#{window_id}' | Out-String).Trim()
    Wait-Until { ((Get-Notes $from) | Where-Object { $_ -match '^%window-add ' }).Count -gt 0 } 3000 | Out-Null
    Start-Sleep -Milliseconds 400
    $notes = Get-Notes $from
    $adds = @($notes | Where-Object { $_ -match '^%window-add ' })
    if ($adds.Count -eq 1 -and $adds[0] -eq "%window-add $w5") { Write-Pass "new-window -d -> '%window-add $w5'" } else { Write-Fail "new-window -d ($w5) -> adds '$($adds -join ' / ')'" }
    if (-not ($notes | Where-Object { $_ -match '^%session-window-changed ' })) { Write-Pass "new-window -d -> no %session-window-changed" } else { Write-Fail "new-window -d changed the current window: $($notes -join ' / ')" }

    Write-Host "`n[6] kill-pane of one of two panes closes no window" -ForegroundColor Yellow
    Invoke-CC "split-window -t '$w1'" | Out-Null
    Start-Sleep -Milliseconds 800
    $p2 = ((Invoke-CC "display-message -p -t '$w1' '#{pane_id}'")[0].Body[0]).Trim()
    $from = (Get-Lines $out).Count - 1
    Invoke-CC "kill-pane -t '$p2'" | Out-Null
    Start-Sleep -Milliseconds 800
    $close = @((Get-Notes $from) | Where-Object { $_ -match 'window-close ' })
    if ($close.Count -eq 0) { Write-Pass "kill-pane $p2 -> no %window-close" } else { Write-Fail "kill-pane $p2 -> '$($close -join ' / ')'" }
    # tmux sends %layout-change for the window that keeps its other pane
    # (layout_close_pane, notify_window "window-layout-changed").
    $lc = @((Get-Notes $from) | Where-Object { $_ -match "^%layout-change $w1 " })
    if ($lc.Count -ge 1 -and $lc[-1] -notmatch '[\[{]') { Write-Pass "kill-pane $p2 -> '$($lc[-1])'" } else { Write-Fail "kill-pane $p2 -> no one-pane %layout-change $w1 in: $((Get-Notes $from) -join ' / ')" }
} catch {
    if ("$_" -ne 'no session') { Write-Fail "exception: $_" }
} finally {
    & $PSMUX -L $ns kill-server 2>$null
    if (-not $p.WaitForExit(3000)) { try { $p.Kill() } catch {} }
    try { $t1.Wait(2000) | Out-Null; $t2.Wait(2000) | Out-Null } catch {}
    $fo.Close(); $fe.Close()
    if ($script:TestsFailed -gt 0) { Write-Host "`n-CC transcript: $out" }
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { Remove-Item -Recurse -Force $work -EA SilentlyContinue; exit 0 }
