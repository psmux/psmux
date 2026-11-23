# Issue #810: control mode capture-pane and pane flags differ from tmux.
#   * capture-pane -P returned the whole screen; tmux returns only a pending
#     incomplete escape sequence (cmd-capture-pane.c cmd_capture_pane_pending),
#     which at a shell prompt is nothing, so the block body is one empty line.
#   * capture-pane -a returned the visible screen. tmux returns the saved
#     normal grid while the alternate screen is on, and with no alternate
#     screen errors "no alternate screen" (or, with -q, an empty body).
#   * #{cursor_flag} was 0 with the cursor visible; tmux reads DECTCEM
#     (format.c format_cb_cursor_flag, MODE_CURSOR).
#   * #{alternate_saved_x}/#{alternate_saved_y} were 0 with nothing saved;
#     tmux prints saved_cx/saved_cy, which screen_init sets to UINT_MAX.
#
# Drives a real `psmux -L <ns> -CC new-session -A -s demo` over pipes, and
# repeats the flag checks over the plain CLI. Private -L namespace, cleaned
# up with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i810_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'i810_' + [guid]::NewGuid().ToString('N').Substring(0, 8)

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
function Desc($blocks) { return ($blocks | ForEach-Object { "$($_.End)[$($_.Body -join '|')]" }) -join ' ' }
function Pane-Text { return (& $PSMUX -L $ns capture-pane -p -t demo | Out-String) }

# A script that writes a marker on the normal screen, switches to the
# alternate screen (DECSET 1049), writes another marker there and waits.
$alt = Join-Path $work 'alt.ps1'
Set-Content -Path $alt -Encoding ASCII -Value @'
[Console]::Out.Write("NORMAL810`r`n")
Start-Sleep -Milliseconds 300
[Console]::Out.Write([char]27 + "[?1049h" + [char]27 + "[H" + "ALTSCREEN810")
[Console]::Out.Flush()
Start-Sleep -Seconds 60
'@

try {
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 10000)) {
        Write-Fail "-CC session did not start"; throw 'no session'
    }
    $r = Invoke-CC "list-panes -F '#{pane_id}'"
    $pane = ($r[0].Body | Select-Object -First 1).Trim()
    Write-Host "pane id: $pane"
    Wait-Until { (Pane-Text).Trim().Length -gt 0 } 10000 | Out-Null
    Start-Sleep -Milliseconds 1500

    Write-Host "`n[1] pane flags at a shell prompt" -ForegroundColor Yellow
    $r = Invoke-CC "display-message -p -t '$pane' 'cursor_flag=#{cursor_flag} alternate_saved_x=#{alternate_saved_x} alternate_saved_y=#{alternate_saved_y}'"
    $body = $r[0].Body -join '|'
    if ($body -match 'cursor_flag=1 ') { Write-Pass "cursor_flag is 1 with the cursor visible" } else { Write-Fail "cursor_flag: '$body'" }
    if ($body -match 'alternate_saved_x=4294967295 alternate_saved_y=4294967295') { Write-Pass "alternate_saved_x/y are UINT_MAX with nothing saved" } else { Write-Fail "alternate_saved_x/y: '$body'" }
    $cli = (& $PSMUX -L $ns display-message -p -t $pane '#{cursor_flag}' | Out-String).Trim()
    if ($cli -eq '1') { Write-Pass "CLI cursor_flag is 1" } else { Write-Fail "CLI cursor_flag '$cli'" }

    Write-Host "`n[2] capture-pane -P returns only a pending partial sequence" -ForegroundColor Yellow
    $r = Invoke-CC "capture-pane -p -P -C -t '$pane'"
    if ($r.Count -eq 1 -and $r[0].End -eq 'end' -and $r[0].Body.Count -eq 1 -and $r[0].Body[0] -eq '') { Write-Pass "capture-pane -p -P -C -> one empty line" } else { Write-Fail "capture-pane -p -P -C -> $(Desc $r)" }
    $r = Invoke-CC "capture-pane -p -P -t '$pane'"
    if ($r.Count -eq 1 -and $r[0].End -eq 'end' -and $r[0].Body.Count -eq 1 -and $r[0].Body[0] -eq '') { Write-Pass "capture-pane -p -P -> one empty line" } else { Write-Fail "capture-pane -p -P -> $(Desc $r)" }
    $cli = & $PSMUX -L $ns capture-pane -p -P -C -t $pane | Out-String
    if ($cli.Trim() -eq '') { Write-Pass "CLI capture-pane -p -P -C prints nothing but the newline" } else { Write-Fail "CLI capture-pane -p -P -C printed '$($cli.Trim())'" }

    Write-Host "`n[3] capture-pane -a with no alternate screen" -ForegroundColor Yellow
    $r = Invoke-CC "capture-pane -p -e -N -a -q -t '$pane'"
    if ($r.Count -eq 1 -and $r[0].End -eq 'end' -and $r[0].Body.Count -eq 1 -and $r[0].Body[0] -eq '') { Write-Pass "capture-pane -a -q -> one empty line" } else { Write-Fail "capture-pane -a -q -> $(Desc $r)" }
    $r = Invoke-CC "capture-pane -p -a -t '$pane'"
    if ($r.Count -eq 1 -and $r[0].End -eq 'error' -and ($r[0].Body -join '|') -eq 'no alternate screen') { Write-Pass "capture-pane -a without -q -> %error 'no alternate screen'" } else { Write-Fail "capture-pane -a without -q -> $(Desc $r)" }
    # Normal capture still works.
    $r = Invoke-CC "capture-pane -p -t '$pane'"
    if ($r.Count -eq 1 -and $r[0].End -eq 'end' -and (($r[0].Body -join "`n").Trim().Length -gt 0)) { Write-Pass "plain capture-pane -p still returns the screen" } else { Write-Fail "plain capture-pane -p -> $(Desc $r)" }

    Write-Host "`n[4] capture-pane -a while a program is on the alternate screen" -ForegroundColor Yellow
    & $PSMUX -L $ns send-keys -t $pane -l "powershell -NoProfile -ExecutionPolicy Bypass -File `"$alt`""
    & $PSMUX -L $ns send-keys -t $pane Enter
    $onAlt = Wait-Until { ((Invoke-CC "display-message -p -t '$pane' '#{alternate_on}'")[0].Body -join '') -eq '1' } 15000
    # The Windows 10 inbox conhost keeps ?1049h to itself and draws its
    # alternate buffer as ordinary output, so there psmux never enters the
    # alternate screen and [4] reads like [3].
    if ($onAlt) { Write-Pass "alternate_on=1" } elseif ($Win10) { Write-Pass "this host kept ?1049h to itself, alternate_on stays 0" } else { Write-Fail "pane never reported alternate_on=1" }
    Wait-Until { (Pane-Text) -match 'ALTSCREEN810' } 5000 | Out-Null
    $r = Invoke-CC "capture-pane -p -e -N -a -q -t '$pane'"
    $txt = $r[0].Body -join "`n"
    if ($r[0].End -eq 'end' -and $txt -match 'NORMAL810' -and $txt -notmatch 'ALTSCREEN810') { Write-Pass "capture-pane -a returns the saved normal screen (NORMAL810, no ALTSCREEN810)" }
    elseif ($Win10 -and $r[0].End -eq 'end' -and $txt.Trim() -eq '') { Write-Pass "capture-pane -a -q -> one empty line, as with no alternate screen" }
    else { Write-Fail "capture-pane -a on the alternate screen -> $($r[0].End): '$($txt.Trim() -replace "`n", ' / ')'" }
    $r = Invoke-CC "capture-pane -p -t '$pane'"
    $txt = $r[0].Body -join "`n"
    if ($txt -match 'ALTSCREEN810') { Write-Pass "capture-pane without -a returns the alternate screen" } else { Write-Fail "capture-pane without -a -> '$($txt.Trim())'" }
    $r = Invoke-CC "display-message -p -t '$pane' '#{alternate_saved_x},#{alternate_saved_y}'"
    $body = $r[0].Body -join ''
    if ($body -match '^(\d+),(\d+)$' -and [int64]$Matches[2] -ge 1 -and [int64]$Matches[2] -lt 1000) { Write-Pass "alternate_saved_x/y hold the saved cursor below the first row ($body)" }
    elseif ($Win10 -and $body -eq '4294967295,4294967295') { Write-Pass "alternate_saved_x/y are UINT_MAX, as with nothing saved" } else { Write-Fail "alternate_saved_x/y on the alternate screen: '$body'" }
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
