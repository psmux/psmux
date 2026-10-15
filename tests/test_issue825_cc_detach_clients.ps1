# Issue #825: control clients and detach-client / sizing.
#
# 1. detach-client from a control client detached EVERY client of the
#    session. tmux (cmd-detach-client.c): no flag detaches the sending
#    client, -t the named client, -a every other client, -s every client of
#    the target session.
# 2. refresh-client -f ignore-size was not honoured and #{session_attached}
#    counted two control clients as 1. tmux leaves an ignore-size client out
#    of the window size (resize.c, CLIENT_IGNORESIZE) and session_attached
#    counts every attached client (session.c session_attached).
# 3. After stdin EOF the control client waited about 2 s before %exit.
#    tmux exits as soon as stdin closes (control.c control_error_callback).
# 4. A detach printed `%exit server exited unexpectedly`, which tmux only
#    prints when the server connection is lost (client.c
#    CLIENT_EXIT_LOST_SERVER). A control client is never "attached" in
#    client.c terms (no MSG_READY), so detach and a destroyed session both
#    print a bare `%exit` (client.c:416-421).
# 5. detach-client -s '=name' was refused; tmux resolves -s as a target
#    session, so the exact match prefix works.
#
# Private -L namespace, cleaned up with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i825_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'i825_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$script:clients = @()

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

# Start a control client on session $name; returns an object with the
# process, its output file and its exit line accessor.
function Start-CC([string]$name, [string]$tag) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $ns -u -CC new-session -A -s $name")
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $out = Join-Path $work "$tag.out"; $err = Join-Path $work "$tag.err"
    $p = [System.Diagnostics.Process]::Start($psi)
    $fo = [System.IO.FileStream]::new($out, 'Create', 'Write', 'ReadWrite', 1)
    $fe = [System.IO.FileStream]::new($err, 'Create', 'Write', 'ReadWrite', 1)
    $t1 = $p.StandardOutput.BaseStream.CopyToAsync($fo, 1)
    $t2 = $p.StandardError.BaseStream.CopyToAsync($fe, 1)
    $c = [pscustomobject]@{ P = $p; Out = $out; Fo = $fo; Fe = $fe; T1 = $t1; T2 = $t2; Tag = $tag }
    $script:clients += $c
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 10000)) {
        throw "control client $tag did not start: $((Get-Lines $out) -join ' / ') $((Get-Lines $err) -join ' / ')"
    }
    return $c
}
function Invoke-CC($c, [string]$line, [int]$waitMs = 7000) {
    $before = (Get-Blocks $c.Out).Count
    try { $c.P.StandardInput.Write("$line`n"); $c.P.StandardInput.Flush() } catch { return $null }
    Wait-Until { (Get-Blocks $c.Out).Count -ge $before + 1 } $waitMs | Out-Null
    $all = Get-Blocks $c.Out
    if ($all.Count -gt $before) { return $all[$before] }
    return $null
}
function Get-Body($c, [string]$line) {
    $b = Invoke-CC $c $line
    if ($null -eq $b) { return '<no reply>' }
    return (($b.Body | ForEach-Object { $_.Trim() }) -join '|')
}
function Get-ExitLine($c) { return @(Get-Lines $c.Out | Where-Object { $_ -match '^%exit' }) | Select-Object -First 1 }
function Test-Exited($c, [int]$ms = 3000) { return $c.P.WaitForExit($ms) }
function Stop-CC($c) {
    try { $c.P.StandardInput.Close() } catch {}
    if (-not $c.P.WaitForExit(4000)) { try { $c.P.Kill() } catch {} }
    try { $c.T1.Wait(2000) | Out-Null; $c.T2.Wait(2000) | Out-Null } catch {}
    try { $c.Fo.Close(); $c.Fe.Close() } catch {}
}

try {
    Write-Host "`n[1] detach-client acts on the sending client only" -ForegroundColor Yellow
    $A = Start-CC 's1' 'a1'
    $B = Start-CC 's1' 'b1'
    $bName = Get-Body $B "display-message -p '#{client_name}'"
    $aName = Get-Body $A "display-message -p '#{client_name}'"
    Write-Host "  A=$aName B=$bName"
    Invoke-CC $B "detach-client" | Out-Null
    if (Test-Exited $B) { Write-Pass "bare detach-client: B exited" } else { Write-Fail "bare detach-client: B still running" }
    if (-not (Test-Exited $A 800)) { Write-Pass "bare detach-client from B: A still attached" } else { Write-Fail "bare detach-client from B: A exited too ($(Get-ExitLine $A))" }
    $r = Get-Body $A "display-message -p 'alive'"
    if ($r -eq 'alive') { Write-Pass "A answers after B detached" } else { Write-Fail "A after B detached -> '$r'" }
    $e = Get-ExitLine $B
    if ($e -eq '%exit') { Write-Pass "detach prints bare %exit (tmux control client)" } else { Write-Fail "detach exit line -> '$e'" }
    if ($B.P.ExitCode -eq 0) { Write-Pass "detached control client exits 0" } else { Write-Fail "detached control client exit code $($B.P.ExitCode)" }
    $bl = Get-Lines $B.Out
    $iEnd = -1; $iExit = -1
    for ($i = 0; $i -lt $bl.Count; $i++) { if ($bl[$i] -match '^%end ') { $iEnd = $i }; if ($bl[$i] -match '^%exit' -and $iExit -lt 0) { $iExit = $i } }
    if ($iEnd -ge 0 -and $iExit -gt $iEnd) { Write-Pass "detach-client's %end comes before %exit" } else { Write-Fail "order: last %end at $iEnd, %exit at $iExit" }

    $A = Start-CC 's1b' 'a2'
    $B = Start-CC 's1b' 'b2'
    $bName = Get-Body $B "display-message -p '#{client_name}'"
    Invoke-CC $B "detach-client -t '$bName'" | Out-Null
    if (Test-Exited $B) { Write-Pass "detach-client -t <self>: B exited" } else { Write-Fail "detach-client -t <self>: B still running" }
    if (-not (Test-Exited $A 800)) { Write-Pass "detach-client -t <B>: A still attached" } else { Write-Fail "detach-client -t <B>: A exited too" }

    $A = Start-CC 's1c' 'a3'
    $B = Start-CC 's1c' 'b3'
    $bName = Get-Body $B "display-message -p '#{client_name}'"
    $r = Invoke-CC $A "detach-client -t '$bName'"
    if (Test-Exited $B) { Write-Pass "A: detach-client -t <B>: B exited" } else { Write-Fail "A: detach-client -t <B>: B still running" }
    if (-not (Test-Exited $A 800)) { Write-Pass "A: detach-client -t <B>: A still attached" } else { Write-Fail "A: detach-client -t <B>: A exited" }

    $A = Start-CC 's1d' 'a4'
    $B = Start-CC 's1d' 'b4'
    Invoke-CC $A "detach-client -a" | Out-Null
    if (Test-Exited $B) { Write-Pass "A: detach-client -a: B exited" } else { Write-Fail "A: detach-client -a: B still running" }
    if (-not (Test-Exited $A 800)) { Write-Pass "A: detach-client -a: A still attached" } else { Write-Fail "A: detach-client -a: A exited" }

    Write-Host "`n[2] ignore-size and session_attached" -ForegroundColor Yellow
    $A = Start-CC 's2' 'a5'
    $B = Start-CC 's2' 'b5'
    Invoke-CC $A "refresh-client -f '!ignore-size'" | Out-Null
    Invoke-CC $A "refresh-client -C '100x30'" | Out-Null
    Invoke-CC $B "refresh-client -f 'ignore-size'" | Out-Null
    Invoke-CC $B "refresh-client -C '80x24'" | Out-Null
    Start-Sleep -Milliseconds 600
    $r = Get-Body $A "display-message -p '#{window_width}x#{window_height} #{session_attached}'"
    $parts = $r -split ' '
    if ($parts[0] -eq '100x30') { Write-Pass "ignore-size client left out of the size: $($parts[0])" } else { Write-Fail "window size with B ignore-size -> '$($parts[0])' (want 100x30)" }
    if ($parts[1] -eq '2') { Write-Pass "session_attached counts both control clients" } else { Write-Fail "session_attached -> '$($parts[1])' (want 2)" }
    $lc = Get-Body $A "list-clients -F '#{client_flags}'"
    if ($lc -match 'ignore-size') { Write-Pass "list-clients shows the ignore-size flag" } else { Write-Fail "client_flags -> '$lc'" }
    Invoke-CC $B "refresh-client -f '!ignore-size'" | Out-Null
    Start-Sleep -Milliseconds 600
    $r = Get-Body $A "display-message -p '#{window_width}x#{window_height}'"
    if ($r -eq '80x24') { Write-Pass "after !ignore-size the smaller client counts again: $r" } else { Write-Fail "after !ignore-size -> '$r' (want 80x24)" }

    Write-Host "`n[3] %exit follows stdin EOF promptly" -ForegroundColor Yellow
    Get-Body $B "display-message -p 'x'" | Out-Null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $B.P.StandardInput.Close()
    $ok = $B.P.WaitForExit(5000); $sw.Stop()
    $ms = $sw.ElapsedMilliseconds
    if ($ok -and $ms -lt 1000) { Write-Pass "stdin EOF -> exit in $ms ms" } else { Write-Fail "stdin EOF -> exit after $ms ms (exited=$ok)" }
    $e = Get-ExitLine $B
    if ($e -eq '%exit') { Write-Pass "stdin EOF prints bare %exit" } else { Write-Fail "stdin EOF exit line -> '$e'" }
    $r = Get-Body $A "display-message -p 'alive'"
    if ($r -eq 'alive') { Write-Pass "A answers after B closed stdin" } else { Write-Fail "A after B EOF -> '$r'" }

    Write-Host "`n[4] CLI detach-client -s and kill-session exit text" -ForegroundColor Yellow
    $A = Start-CC 's4' 'a6'
    & $PSMUX -L $ns detach-client -s s4 2>&1 | Out-Null
    if (Test-Exited $A) { Write-Pass "CLI detach-client -s s4: A exited" } else { Write-Fail "CLI detach-client -s s4: A still running" }
    $e = Get-ExitLine $A
    if ($e -eq '%exit') { Write-Pass "CLI detach prints bare %exit" } else { Write-Fail "CLI detach exit line -> '$e'" }
    if ($A.P.ExitCode -eq 0) { Write-Pass "CLI detach: exit code 0" } else { Write-Fail "CLI detach: exit code $($A.P.ExitCode)" }
    & $PSMUX -L $ns has-session -t s4 2>$null
    if ($LASTEXITCODE -eq 0) { Write-Pass "session s4 survives the detach" } else { Write-Fail "session s4 gone after detach" }

    $K = Start-CC 'k1' 'k1'
    & $PSMUX -L $ns kill-session -t k1 2>&1 | Out-Null
    if (Test-Exited $K) { Write-Pass "kill-session: control client exited" } else { Write-Fail "kill-session: control client still running" }
    $e = Get-ExitLine $K
    if ($e -eq '%exit') { Write-Pass "kill-session prints bare %exit" } else { Write-Fail "kill-session exit line -> '$e'" }

    Write-Host "`n[5] detach-client -s accepts an exact match target" -ForegroundColor Yellow
    $D = Start-CC 'dd' 'd1'
    $o = (& $PSMUX -L $ns detach-client -s '=dd' 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -eq 0 -and $o -eq '') { Write-Pass "detach-client -s =dd accepted" } else { Write-Fail "detach-client -s =dd -> rc $LASTEXITCODE '$o'" }
    if (Test-Exited $D) { Write-Pass "detach-client -s =dd detached the client" } else { Write-Fail "detach-client -s =dd: client still running" }

    Write-Host "`n[6] kill-server keeps its reason" -ForegroundColor Yellow
    $S = Start-CC 'sv' 'sv1'
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    if (Test-Exited $S 5000) { Write-Pass "kill-server: control client exited" } else { Write-Fail "kill-server: control client still running" }
    $e = Get-ExitLine $S
    if ($e -eq '%exit server exited') { Write-Pass "kill-server prints '%exit server exited'" } else { Write-Fail "kill-server exit line -> '$e'" }
} catch {
    Write-Fail "exception: $_"
} finally {
    & $PSMUX -L $ns kill-server 2>$null
    foreach ($c in $script:clients) { Stop-CC $c }
    if ($script:TestsFailed -gt 0) { Write-Host "`ntranscripts: $work" }
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit $script:TestsFailed } else { Remove-Item -Recurse -Force $work -EA SilentlyContinue; exit 0 }
