# Issue #799: in control mode (-CC) `send-keys -H ...` and `send-keys -l ...`
# answered `%begin` / `command timed out` / `%error` about 5 s later although
# the keys were delivered. tmux answers an empty `%begin` / `%end` block as
# soon as the keys are written (cmd-send-keys.c returns CMD_RETURN_NORMAL,
# cmd-queue.c cmdq_fire_command then prints the `end` guard).
#
# This suite drives a real `psmux -L <ns> -CC new-session -A -s demo` over
# pipes and, for every send-keys form, asserts the block ends in `%end`, has
# an empty body, and arrives within 1 s. It also checks that the keys really
# arrive in the pane, and that the same forms over the plain CLI and the raw
# TCP route keep working.
#
# Everything runs under a private -L namespace and is cleaned up with
# `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i799_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'i799_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$LimitMs = 1000

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

# Parse the -CC output into command reply blocks (%begin .. %end/%error).
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

# Send one line, wait for $expect new reply blocks, return them with latency.
function Invoke-CC([string]$line, [int]$expect = 1, [int]$waitMs = 7000) {
    $before = (Get-Blocks $out).Count
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p.StandardInput.Write("$line`n"); $p.StandardInput.Flush()
    Wait-Until { (Get-Blocks $out).Count -ge $before + $expect } $waitMs | Out-Null
    $ms = $sw.ElapsedMilliseconds
    $all = Get-Blocks $out
    $new = @(); for ($i = $before; $i -lt $all.Count; $i++) { $new += $all[$i] }
    return [pscustomobject]@{ Ms = $ms; Blocks = $new }
}

# Assert $n blocks, each %end, the send-keys ones empty, all within the limit.
function Assert-Reply([string]$label, $r, [int]$n = 1, [string[]]$bodies = $null) {
    $ok = $r.Blocks.Count -eq $n
    $desc = ($r.Blocks | ForEach-Object { "$($_.End)[$($_.Body -join '|')]" }) -join ' '
    for ($i = 0; $ok -and $i -lt $r.Blocks.Count; $i++) {
        $b = $r.Blocks[$i]
        $want = if ($bodies) { $bodies[$i] } else { '' }
        if ($b.End -ne 'end' -or (($b.Body -join '|') -ne $want)) { $ok = $false }
    }
    if ($ok -and $r.Ms -lt $LimitMs) { Write-Pass "$label -> $desc in $($r.Ms) ms" }
    else { Write-Fail "$label -> expected $n %end block(s) under $LimitMs ms, got '$desc' in $($r.Ms) ms" }
}

function Pane-Text { return (& $PSMUX -L $ns capture-pane -p -t demo | Out-String) }

try {
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 10000)) {
        Write-Fail "-CC session did not start"; throw 'no session'
    }
    $r = Invoke-CC "list-panes -F '#{pane_id}'"
    $pane = ($r.Blocks[0].Body | Select-Object -First 1).Trim()
    Write-Host "pane id: $pane"
    # Let the shell reach its prompt so typed text is echoed.
    Wait-Until { (Pane-Text).Trim().Length -gt 0 } 10000 | Out-Null
    Start-Sleep -Milliseconds 800

    Write-Host "`n[1] control mode send-keys forms" -ForegroundColor Yellow
    Assert-Reply 'display-message -p settled' (Invoke-CC "display-message -p 'settled'") 1 @('settled')

    Assert-Reply 'send-keys -H' (Invoke-CC "send-keys -t '$pane' -H 65 63 68 6f 20 48 58 37 39 39 0d")
    if (Wait-Until { (Pane-Text) -match '(?m)^HX799\s*$' } 5000) { Write-Pass "-H bytes ran 'echo HX799'" } else { Write-Fail "-H bytes did not reach the pane" }

    Assert-Reply 'send-keys -l' (Invoke-CC "send-keys -t '$pane' -l 'echo LIT799'")
    Assert-Reply 'send-keys Enter' (Invoke-CC "send-keys -t '$pane' Enter")
    if (Wait-Until { (Pane-Text) -match '(?m)^LIT799\s*$' } 5000) { Write-Pass "-l text ran 'echo LIT799'" } else { Write-Fail "-l text did not reach the pane" }

    Assert-Reply 'send-keys plain keys' (Invoke-CC "send-keys -t '$pane' 'echo PLAIN799' Enter")
    if (Wait-Until { (Pane-Text) -match '(?m)^PLAIN799\s*$' } 5000) { Write-Pass "plain keys ran 'echo PLAIN799'" } else { Write-Fail "plain keys did not reach the pane" }

    Assert-Reply 'command list display ; send-keys -H ; display' (Invoke-CC "display-message -p 'one' ; send-keys -t '$pane' -H 0d ; display-message -p 'after-send'" 3) 3 @('one', '', 'after-send')

    Assert-Reply 'send-keys -N 3 -l' (Invoke-CC "send-keys -t '$pane' -N 3 -l z")
    Assert-Reply 'send-keys -N 2 -H' (Invoke-CC "send-keys -t '$pane' -N 2 -H 79")
    Assert-Reply 'send-keys -N 2 named key' (Invoke-CC "send-keys -t '$pane' -N 2 BSpace")
    Assert-Reply 'send-keys -l without -t' (Invoke-CC "send-keys -l q")
    Assert-Reply 'send-keys -H without -t' (Invoke-CC "send-keys -H 71")
    Assert-Reply 'send-keys Escape' (Invoke-CC "send-keys -t '$pane' Escape")
    Assert-Reply 'send-keys -R' (Invoke-CC "send-keys -t '$pane' -R")

    # Consecutive sends on one line are merged into one pane write (iTerm2
    # sends an arrow key as `send 0x1b 0x5b ; send -l A`), but tmux still
    # answers one block per command (cmd-queue.c:592 and :652, one guard per
    # item), and a client matches replies to commands by count.
    Assert-Reply 'merged send-keys -l ; send-keys -l' (Invoke-CC "send-keys -t '$pane' -l 'echo ME' ; send-keys -t '$pane' -l 'RGED799'" 2) 2 @('', '')
    Assert-Reply 'merged send -H ; send -l ; send Enter style 0xd' (Invoke-CC "send -t '$pane' -H 59 ; send -lt '$pane' Z ; send -t '$pane' 0xd" 3) 3 @('', '', '')
    if (Wait-Until { (Pane-Text) -match '(?m)^MERGED799YZ\s*$' } 5000) { Write-Pass "merged sends ran 'echo MERGED799YZ'" } else { Write-Fail "merged sends did not reach the pane" }
    Assert-Reply 'merged sends then display' (Invoke-CC "send-keys -t '$pane' -l a ; send-keys -t '$pane' -H 08 ; display-message -p 'm3'" 3) 3 @('', '', 'm3')

    # -N repeats and `--` ends the options on this route too.
    Assert-Reply 'send-keys -N 3 -l then -N 2 -H then -- -dash' (Invoke-CC "send-keys -t '$pane' -l 'echo N' ; send-keys -t '$pane' -N 3 -l a ; send-keys -t '$pane' -N 2 -H 62 ; send-keys -t '$pane' -l -- -X ; send-keys -t '$pane' Enter" 5) 5 @('', '', '', '', '')
    if (Wait-Until { (Pane-Text) -match '(?m)^Naaabb-X\s*$' } 5000) { Write-Pass "-N counts and -- operand reached the pane ('Naaabb-X')" } else { Write-Fail "-N / -- keys wrong in the pane" }

    # -X outside a mode is tmux's error (cmd-send-keys.c:193), not typed text.
    $r = Invoke-CC "send-keys -t '$pane' -X cancel"
    $b = $r.Blocks | Select-Object -First 1
    if ($r.Blocks.Count -eq 1 -and $b.End -eq 'error' -and ($b.Body -join '|') -eq 'not in a mode' -and $r.Ms -lt $LimitMs) { Write-Pass "send-keys -X outside a mode -> %error 'not in a mode' in $($r.Ms) ms" }
    else { Write-Fail "send-keys -X outside a mode -> '$($b.End)[$($b.Body -join '|')]' in $($r.Ms) ms" }
    Write-Host "`n[2] copy mode commands via send-keys -X" -ForegroundColor Yellow
    # copy-mode on the control route is deliberately a no-op (iTerm2 keeps its
    # own copy mode), so enter it over the CLI.
    & $PSMUX -L $ns copy-mode -t $pane
    $r = Invoke-CC "display-message -p -t '$pane' '#{pane_in_mode}'"
    if ($r.Blocks.Count -eq 1 -and $r.Blocks[0].Body -contains '1') { Write-Pass "pane is in copy mode" } else { Write-Fail "pane not in copy mode: $($r.Blocks[0].Body)" }
    Assert-Reply 'send-keys -X cursor-up' (Invoke-CC "send-keys -t '$pane' -X cursor-up")
    Assert-Reply 'send-keys -N 2 -X cursor-up' (Invoke-CC "send-keys -t '$pane' -N 2 -X cursor-up")
    Assert-Reply 'send-keys -X cancel' (Invoke-CC "send-keys -t '$pane' -X cancel")
    $r = Invoke-CC "display-message -p -t '$pane' '#{pane_in_mode}'"
    if ($r.Blocks.Count -eq 1 -and $r.Blocks[0].Body -contains '0') { Write-Pass "copy mode left after -X cancel" } else { Write-Fail "still in copy mode: $($r.Blocks[0].Body)" }

    Write-Host "`n[3] a second command after the send-keys burst is still answered in order" -ForegroundColor Yellow
    Assert-Reply 'display-message -p tail' (Invoke-CC "display-message -p 'tail799'") 1 @('tail799')

    Write-Host "`n[3b] show-buffer of a missing buffer answers tmux's error at once" -ForegroundColor Yellow
    # cmd-save-buffer.c:92: cmdq_error(item, "no buffer %s", bufname).
    $r = Invoke-CC "show-buffer -b nosuch799"
    $b = $r.Blocks | Select-Object -First 1
    if ($r.Blocks.Count -eq 1 -and $b.End -eq 'error' -and ($b.Body -join '|') -eq 'no buffer nosuch799' -and $r.Ms -lt $LimitMs) { Write-Pass "show-buffer -b nosuch799 -> %error 'no buffer nosuch799' in $($r.Ms) ms" }
    else { Write-Fail "show-buffer -b nosuch799 -> '$($b.End)[$($b.Body -join '|')]' in $($r.Ms) ms" }

    Write-Host "`n[4] plain CLI route" -ForegroundColor Yellow
    foreach ($c in @(
        @{ L = 'CLI send-keys -H'; A = @('send-keys', '-t', $pane, '-H', '65', '63', '68', '6f', '20', '43', '4c', '49', '0d') },
        @{ L = 'CLI send-keys -l'; A = @('send-keys', '-t', $pane, '-l', 'echo CLILIT') },
        @{ L = 'CLI send-keys Enter'; A = @('send-keys', '-t', $pane, 'Enter') },
        @{ L = 'CLI send-keys -N 2 -l'; A = @('send-keys', '-t', $pane, '-N', '2', '-l', 'k') },
        @{ L = 'CLI send-keys Escape'; A = @('send-keys', '-t', $pane, 'Escape') }
    )) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $o = & $PSMUX -L $ns @($c.A) 2>&1 | Out-String
        $code = $LASTEXITCODE; $ms = $sw.ElapsedMilliseconds
        if ($code -eq 0 -and $o.Trim() -eq '' -and $ms -lt 3000) { Write-Pass "$($c.L) exit 0 in $ms ms" } else { Write-Fail "$($c.L) exit $code in $ms ms, output '$($o.Trim())'" }
    }
    if (Wait-Until { (Pane-Text) -match '(?m)^CLI\s*$' } 5000) { Write-Pass "CLI -H bytes ran 'echo CLI'" } else { Write-Fail "CLI -H bytes did not reach the pane" }
    if (Wait-Until { (Pane-Text) -match '(?m)^CLILIT\s*$' } 5000) { Write-Pass "CLI -l text ran 'echo CLILIT'" } else { Write-Fail "CLI -l text did not reach the pane" }

    Write-Host "`n[5] raw TCP route" -ForegroundColor Yellow
    $dir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE '.psmux' }
    $port = (Get-Content (Join-Path $dir "${ns}__demo.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $dir "${ns}__demo.key") -Raw).Trim()
    function Send-Tcp([string]$cmd) {
        $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
        $tcp.ReceiveTimeout = 7000
        $s = $tcp.GetStream()
        $w = [System.IO.StreamWriter]::new($s); $rd = [System.IO.StreamReader]::new($s)
        $w.Write("AUTH $key`n"); $w.Flush(); $null = $rd.ReadLine()
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $w.Write("$cmd`n"); $w.Flush()
        $resp = ''; try { $resp = $rd.ReadToEnd() } catch { $resp = 'TIMEOUT' }
        $ms = $sw.ElapsedMilliseconds; $tcp.Close()
        return [pscustomobject]@{ Ms = $ms; Resp = $resp }
    }
    foreach ($c in @(
        @{ L = 'TCP send-keys -H'; C = "send-keys -t $pane -H 65 63 68 6f 20 54 43 50 0d" },
        @{ L = 'TCP send-keys -l'; C = "send-keys -t $pane -l 'echo TCPLIT'" },
        @{ L = 'TCP send-keys Enter'; C = "send-keys -t $pane Enter" }
    )) {
        $r = Send-Tcp $c.C
        if ($r.Resp -ne 'TIMEOUT' -and $r.Resp -notmatch 'error|timed out' -and $r.Ms -lt 3000) { Write-Pass "$($c.L) closed in $($r.Ms) ms, reply '$($r.Resp.Trim())'" } else { Write-Fail "$($c.L) reply '$($r.Resp.Trim())' in $($r.Ms) ms" }
    }
    if (Wait-Until { (Pane-Text) -match '(?m)^TCP\s*$' } 5000) { Write-Pass "TCP -H bytes ran 'echo TCP'" } else { Write-Fail "TCP -H bytes did not reach the pane" }
    if (Wait-Until { (Pane-Text) -match '(?m)^TCPLIT\s*$' } 5000) { Write-Pass "TCP -l text ran 'echo TCPLIT'" } else { Write-Fail "TCP -l text did not reach the pane" }

    Write-Host "`n[6] no stray %error anywhere in the -CC stream" -ForegroundColor Yellow
    $errs = @(Get-Lines $out | Where-Object { $_ -match '^%error' })
    if ($errs.Count -eq 2) { Write-Pass "only the two expected %error lines (-X outside a mode, show-buffer)" } else { Write-Fail "$($errs.Count) %error line(s), expected 2" }
    $timeouts = @(Get-Lines $out | Where-Object { $_ -match 'command timed out' })
    if ($timeouts.Count -eq 0) { Write-Pass "no 'command timed out' bodies" } else { Write-Fail "$($timeouts.Count) 'command timed out' bodies" }
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
