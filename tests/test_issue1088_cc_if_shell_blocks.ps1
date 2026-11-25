# Issue #1088: over -CC, `if-shell -F 1 'display-message -p yes'` answered ONE
# %begin/%end block holding `yes`. tmux 3.8 answers an empty block for the
# if-shell item and then one block per command of the chosen branch
# (cmd-if-shell.c:99-100 and :169-170 queue the branch as new items after the
# if-shell; cmd-queue.c:592 and :652 guard every item), so a branch of n
# commands answers n+1 blocks. Clients such as agent-manager count blocks to
# match replies to the commands they sent.
#
# This suite drives a real `psmux -L <ns> -CC new-session -A -s demo` over
# pipes and checks block count, order, bodies, numbering and flags for -F true
# and false, else branches, two command branches (quoted and in braces), a
# shell condition (its block ends before the shell runs), nested if-shell,
# errors in and of the branch, and `-b`. It also checks that the plain CLI
# if-shell keeps printing the branch output.
#
# Everything runs under a private -L namespace and PSMUX_DATA_DIR and is
# cleaned up with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i1088_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $work 'data'
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$ns = 'i1088_' + [guid]::NewGuid().ToString('N').Substring(0, 8)

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

# Parse the -CC output into reply blocks: number, flags, body, end kind.
function Get-Blocks([string]$path) {
    $blocks = @(); $cur = $null
    foreach ($l in (Get-Lines $path)) {
        if ($l -match '^%begin (\d+) (\d+) (\d+)$') { $cur = @{ Num = [int64]$Matches[2]; Flags = $Matches[3]; Body = @(); End = $null }; continue }
        if ($null -ne $cur) {
            if ($l -match '^%(end|error) (\d+) (\d+) (\d+)$') {
                $cur.End = $Matches[1]
                if ([int64]$Matches[3] -ne $cur.Num -or $Matches[4] -ne $cur.Flags) { $cur.End = 'mismatched' }
                $blocks += [pscustomobject]$cur; $cur = $null; continue
            }
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

# Send one line followed by a sync command, and return the blocks the line
# answered: everything before the sync block. A client that counts blocks
# sees exactly this.
$script:syncN = 0
function Invoke-CC([string]$line, [int]$waitMs = 10000) {
    $script:syncN++
    $tag = "sync1088x$($script:syncN)"
    $before = (Get-Blocks $out).Count
    $p.StandardInput.Write("$line`ndisplay-message -p $tag`n"); $p.StandardInput.Flush()
    $ok = Wait-Until { @((Get-Blocks $out) | Where-Object { ($_.Body -join '|') -eq $tag }).Count -gt 0 } $waitMs
    $all = Get-Blocks $out
    $new = @()
    for ($i = $before; $i -lt $all.Count; $i++) {
        if (($all[$i].Body -join '|') -eq $tag) { break }
        $new += $all[$i]
    }
    if (-not $ok) { $new += [pscustomobject]@{ Num = -1; Flags = '?'; Body = @('NO SYNC'); End = 'timeout' } }
    return , $new
}

function Describe($blocks) { return (($blocks | ForEach-Object { "$($_.End)/$($_.Flags)[$($_.Body -join '|')]" }) -join ' ') }

# Assert the blocks: one spec per block, 'end:body' or 'error:body' with an
# optional '@flags' (default 1); numbers must rise by one each.
function Assert-Blocks([string]$label, $blocks, [string[]]$spec) {
    $ok = $blocks.Count -eq $spec.Count
    for ($i = 0; $ok -and $i -lt $spec.Count; $i++) {
        $s = $spec[$i]; $flags = '1'
        if ($s -match '^(.*)@([01])$') { $s = $Matches[1]; $flags = $Matches[2] }
        $kind, $body = $s -split ':', 2
        $b = $blocks[$i]
        if ($b.End -ne $kind -or ($b.Body -join '|') -ne $body -or $b.Flags -ne $flags) { $ok = $false }
        if ($i -gt 0 -and $b.Num -ne $blocks[$i - 1].Num + 1) { $ok = $false }
    }
    if ($ok) { Write-Pass "$label -> $($blocks.Count) block(s): $(Describe $blocks)" }
    else { Write-Fail "$label -> expected $($spec.Count) block(s) [$($spec -join ' ')], got $($blocks.Count): $(Describe $blocks)" }
}

try {
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 15000)) {
        Write-Fail "-CC session did not start"; throw 'no session'
    }
    Start-Sleep -Milliseconds 500

    Write-Host "`n[1] -F conditions" -ForegroundColor Yellow
    Assert-Blocks "plain display-message" (Invoke-CC "display-message -p plain") @('end:plain')
    # The issue's example: agent-manager expects exactly two blocks.
    Assert-Blocks "if-shell -F 1 'display-message -p yes'" (Invoke-CC "if-shell -F 1 'display-message -p yes'") @('end:', 'end:yes')
    Assert-Blocks "if-shell -F 0 without else" (Invoke-CC "if-shell -F 0 'display-message -p yes'") @('end:')
    Assert-Blocks "if-shell -F 0 else branch" (Invoke-CC "if-shell -F 0 'display-message -p yes' 'display-message -p no'") @('end:', 'end:no')
    Assert-Blocks "if-shell -F '#{==:a,a}' format true" (Invoke-CC "if-shell -F '#{==:a,a}' 'display-message -p fmt'") @('end:', 'end:fmt')
    # n commands in the branch: n+1 blocks.
    Assert-Blocks "two command branch (quoted)" (Invoke-CC "if-shell -F 1 'display-message -p a ; display-message -p b'") @('end:', 'end:a', 'end:b')
    Assert-Blocks "two command branch (braces)" (Invoke-CC "if-shell -F 1 { display-message -p a ; display-message -p b }") @('end:', 'end:a', 'end:b')
    Assert-Blocks "empty branch output (set-option)" (Invoke-CC "if-shell -F 1 'set -g @i1088 v'") @('end:', 'end:')

    Write-Host "`n[2] shell conditions" -ForegroundColor Yellow
    Assert-Blocks "if-shell 'exit 0' (true)" (Invoke-CC "if-shell 'exit 0' 'display-message -p shyes' 'display-message -p shno'") @('end:', 'end:shyes')
    Assert-Blocks "if-shell 'exit 1' (else)" (Invoke-CC "if-shell 'exit 1' 'display-message -p shyes' 'display-message -p shno'") @('end:', 'end:shno')
    Assert-Blocks "if-shell 'exit 1' without else" (Invoke-CC "if-shell 'exit 1' 'display-message -p shyes'") @('end:')

    # The if-shell block ends before the shell runs; the branch follows when
    # it exits (about 2 s for ping -n 3).
    $before = (Get-Blocks $out).Count
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p.StandardInput.Write("if-shell 'ping -n 3 127.0.0.1' 'display-message -p slowyes'`n"); $p.StandardInput.Flush()
    Wait-Until { (Get-Blocks $out).Count -ge $before + 1 } 10000 | Out-Null
    $tEnd = $sw.ElapsedMilliseconds
    Wait-Until { (Get-Blocks $out).Count -ge $before + 2 } 15000 | Out-Null
    $tBranch = $sw.ElapsedMilliseconds
    $all = Get-Blocks $out
    $first = if ($all.Count -gt $before) { $all[$before] } else { $null }
    $second = if ($all.Count -gt $before + 1) { $all[$before + 1] } else { $null }
    if ($first -and $first.End -eq 'end' -and $first.Body.Count -eq 0 -and $second -and ($second.Body -join '|') -eq 'slowyes' -and $tEnd -lt 1000 -and ($tBranch - $tEnd) -gt 1000) {
        Write-Pass "slow shell condition: if-shell block ended at $tEnd ms, branch block at $tBranch ms"
    } else {
        Write-Fail "slow shell condition: if-shell block at $tEnd ms [$(if ($first) { Describe @($first) })], branch at $tBranch ms [$(if ($second) { Describe @($second) })]"
    }

    Write-Host "`n[3] nesting, order and the rest of the line" -ForegroundColor Yellow
    Assert-Blocks "nested if-shell -F" (Invoke-CC "if-shell -F 1 `"if-shell -F 1 'display-message -p inner'`"") @('end:', 'end:', 'end:inner')
    Assert-Blocks "nested shell in -F" (Invoke-CC "if-shell -F 1 `"if-shell 'exit 0' 'display-message -p inner2'`"") @('end:', 'end:', 'end:inner2')
    # The branch runs right after the if-shell, before the rest of the line.
    Assert-Blocks "branch before the rest of the line" (Invoke-CC "if-shell -F 1 'display-message -p x' ; display-message -p after") @('end:', 'end:x', 'end:after')
    Assert-Blocks "shell branch before the rest of the line" (Invoke-CC "if-shell 'exit 0' 'display-message -p sx' ; display-message -p safter") @('end:', 'end:sx', 'end:safter')

    Write-Host "`n[4] errors" -ForegroundColor Yellow
    # A branch that does not parse is the if-shell's error; nothing runs.
    Assert-Blocks "unknown command in -F branch" (Invoke-CC "if-shell -F 1 'display-message -p a ; bogus-cmd1088'") @('error:unknown command: bogus-cmd1088')
    # A failing branch command drops the rest of the branch; the line goes on.
    $r = Invoke-CC "if-shell -F 1 'display-message -p a ; display-message -t nosuch1088:9 -p b ; display-message -p c' ; display-message -p outer"
    Assert-Blocks "failure inside the branch" $r @('end:', 'end:a', 'error:can''t find session: nosuch1088', 'end:outer')
    Assert-Blocks "usage error" (Invoke-CC "if-shell -F 1") @('error:usage: if-shell [-bF] [-t target-pane] shell-command command [command]')

    Write-Host "`n[5] if-shell -b" -ForegroundColor Yellow
    # The branch is appended with a fresh state: flags 0, after the line.
    Assert-Blocks "if-shell -b branch has flags 0" (Invoke-CC "if-shell -b 'exit 0' 'display-message -p bg' ; display-message -p afterbg") @('end:', 'end:afterbg', 'end:bg@0')

    Write-Host "`n[6] non control mode if-shell is unchanged" -ForegroundColor Yellow
    $cli = (& $PSMUX -L $ns if-shell -F 1 'display-message -p cliyes' 2>&1 | Out-String).Trim()
    if ($cli -eq 'cliyes') { Write-Pass "CLI if-shell -F prints 'cliyes'" } else { Write-Fail "CLI if-shell -F printed '$cli'" }
    $cli2 = ((& $PSMUX -L $ns if-shell -F 1 'display-message -p a1 ; display-message -p b1' 2>&1) -join '|').Trim()
    if ($cli2 -eq 'a1|b1') { Write-Pass "CLI two command branch prints a1, b1" } else { Write-Fail "CLI two command branch printed '$cli2'" }
    & $PSMUX -L $ns if-shell 'exit 0' 'set -g @cli1088 shell' 2>&1 | Out-Null
    $v = (& $PSMUX -L $ns show -gv '@cli1088' 2>&1 | Out-String).Trim()
    if ($v -eq 'shell') { Write-Pass "CLI shell if-shell ran its branch" } else { Write-Fail "CLI shell if-shell branch: @cli1088='$v'" }
} finally {
    try { $p.StandardInput.Close() } catch {}
    & $PSMUX -L $ns kill-server 2>$null | Out-Null
    if (-not $p.WaitForExit(5000)) { try { $p.Kill() } catch {} }
    try { $fo.Close(); $fe.Close() } catch {}
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
