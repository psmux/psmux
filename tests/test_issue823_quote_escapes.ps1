# Issue #823: argument parsing differs from tmux.
#   * 'a'\''b' (adjacent quoted and backslash escaped parts) must be ONE
#     argument a'b. tmux's lexer treats `\` outside single quotes as an escape
#     and keeps building the same token (cmd-parse.y yylex_token).
#   * In control mode, inside double quotes tmux decodes \n \r \t \e \a \b \f
#     \v \s, octal \NNN, \uXXXX, \UXXXXXXXX, and \c -> c for anything else
#     (cmd-parse.y yylex_token_escape), so set-buffer can carry a newline.
# Windows paths must keep working on every route touched: unquoted and
# single quoted backslashes stay literal everywhere, and config files keep
# the Windows friendly rule inside double quotes (only \" and \\ decode),
# because documented configs say set -g default-shell "C:\Program Files\...".
#
# Private -L namespace, cleaned up with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Show([string]$s) { return ($s -replace "`t", '<TAB>' -replace "`e", '<ESC>' -replace "`n", '<LF>') }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i823_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'i823_' + [guid]::NewGuid().ToString('N').Substring(0, 8)

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
# One set-buffer line, then the buffer's body lines joined with LF.
function CC-Buffer([string]$name, [string]$valueSpelling) {
    Invoke-CC "set-buffer -b $name $valueSpelling" | Out-Null
    $r = Invoke-CC "show-buffer -b $name"
    if ($r.Count -lt 1) { return '<no reply>' }
    return ($r[0].Body -join "`n")
}
function Check([string]$what, [string]$got, [string]$want) {
    if ($got -ceq $want) { Write-Pass "$what -> [$(Show $got)]" } else { Write-Fail "$what -> [$(Show $got)], want [$(Show $want)]" }
}
function Pane-Text { return (& $PSMUX -L $ns capture-pane -p -t demo | Out-String) }

try {
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 10000)) {
        Write-Fail "-CC session did not start"; throw 'no session'
    }
    $pane = ((Invoke-CC "list-panes -F '#{pane_id}'")[0].Body | Select-Object -First 1).Trim()
    $win = ((Invoke-CC "display-message -p '#{window_id}'")[0].Body | Select-Object -First 1).Trim()
    Write-Host "pane $pane window $win"

    Write-Host "`n[1] control mode: '\'' joins one argument" -ForegroundColor Yellow
    Invoke-CC @"
rename-window -t '$win' 'a'\''b'
"@ | Out-Null
    $r = Invoke-CC "display-message -p '#{window_name}'"
    Check "rename-window 'a'\''b'" ($r[0].Body -join '|') "a'b"
    Invoke-CC @"
rename-window -t '$win' 'it'"'"'s'
"@ | Out-Null
    $r = Invoke-CC "display-message -p '#{window_name}'"
    Check "rename-window 'it'`"'`"'s'" ($r[0].Body -join '|') "it's"
    Check 'unquoted a\"b' (CC-Buffer 'uq1' 'a\"b') 'a"b'

    Write-Host "`n[2] control mode: tmux escapes inside double quotes" -ForegroundColor Yellow
    Check 'issue line mq' (CC-Buffer 'mq' '"L1\nL2\tT\u0041\e[0m\$HOME\~x\101"') "L1`nL2`tTA`e[0m`$HOME~xA"
    Check '\u000a and \u0009' (CC-Buffer 'mq2' '"u1\u000au2\u0009T"') "u1`nu2`tT"
    Check '\\ and \" and \s' (CC-Buffer 'mq3' '"\\back \"dq\"\sx"') '\back "dq" x'
    Check '\U0001F600' (CC-Buffer 'mq4' '"<\U0001F600>"') ("<" + [char]::ConvertFromUtf32(0x1F600) + ">")
    Check 'octal UTF-8 bytes \303\251' (CC-Buffer 'mq5' '"caf\303\251"') ("caf" + [char]0xE9)
    Check 'semicolon inside quotes stays data' (CC-Buffer 'mq6' '"x ; y"') 'x ; y'
    Check 'single quotes stay literal' (CC-Buffer 'mq7' "'L1\nL2'") 'L1\nL2'

    Write-Host "`n[3] control mode: Windows paths" -ForegroundColor Yellow
    Check 'unquoted C:\Users\x\new' (CC-Buffer 'wp1' 'C:\Users\x\new') 'C:\Users\x\new'
    Check "single quoted 'C:\Users\x\new'" (CC-Buffer 'wp2' "'C:\Users\x\new'") 'C:\Users\x\new'
    Check 'unquoted \\server\share' (CC-Buffer 'wp3' '\\server\share') '\\server\share'
    Check "single quoted '\\server\share\'" (CC-Buffer 'wp4' "'\\server\share\'") '\\server\share\'
    Check 'double quoted "C:\\Users\\x"' (CC-Buffer 'wp5' '"C:\\Users\\x"') 'C:\Users\x'

    Write-Host "`n[4] control mode: a multi-line paste through a buffer" -ForegroundColor Yellow
    Invoke-CC "set-buffer -b mp `"echo pl1-823\necho pl2-823\n`" ; paste-buffer -p -d -b 'mp' -t '$pane'" 2 | Out-Null
    $ok = Wait-Until { $t = Pane-Text; ($t -match '(?m)^pl1-823\s*$') -and ($t -match '(?m)^pl2-823\s*$') } 8000
    if ($ok) { Write-Pass "pasted buffer ran two commands (pl1-823, pl2-823)" } else { Write-Fail "paste did not run two lines: $(Show (Pane-Text))" }
    if ((Pane-Text) -match [regex]::Escape('pl1-823\necho')) { Write-Fail "pane shows the literal \n text" } else { Write-Pass "no literal \n text in the pane" }

    Write-Host "`n[5] source-file: '\'' joins, Windows paths unchanged" -ForegroundColor Yellow
    $conf = Join-Path $work 'i823.conf'
    $confText = @'
set -g @o1 'a'\''b'
set -g @o2 "C:\new\temp\bin"
set -g @o3 'C:\Users\x\new'
set -g @o4 C:\Users\x\new
set -g @o5 \\server\share
set -g @o6 "C:\\Users\\x"
set -g @o7 "C:\Program Files\Git\bin\bash.exe"
set -g @o8 'it'"'"'s'
'@
    [System.IO.File]::WriteAllText($conf, $confText, $utf8)
    & $PSMUX -L $ns source-file $conf
    $want = [ordered]@{ '@o1' = "a'b"; '@o2' = 'C:\new\temp\bin'; '@o3' = 'C:\Users\x\new'; '@o4' = 'C:\Users\x\new'
        '@o5' = '\\server\share'; '@o6' = 'C:\Users\x'; '@o7' = 'C:\Program Files\Git\bin\bash.exe'; '@o8' = "it's" }
    foreach ($k in $want.Keys) { Check "source-file $k" ((& $PSMUX -L $ns show-options -gv $k) -join "`n") $want[$k] }

    Write-Host "`n[6] CLI argv (shell already split): values round trip byte exact" -ForegroundColor Yellow
    $vals = [ordered]@{ '@c1' = 'C:\new\temp'; '@c2' = '\\server\share\'; '@c3' = "it's"; '@c4' = 'a\"b'
        '@c5' = 'C:\Program Files\x'; '@c6' = "a\'b"; '@c7' = 'x\ny' }
    foreach ($k in $vals.Keys) {
        & $PSMUX -L $ns set -g $k $vals[$k]
        Check "CLI set $k" ((& $PSMUX -L $ns show-options -gv $k) -join "`n") $vals[$k]
    }

    Write-Host "`n[7] nested command strings (if-shell)" -ForegroundColor Yellow
    & $PSMUX -L $ns if-shell "exit 0" "set -g @n1 'a'\''b'"
    Check "if-shell branch 'a'\''b'" ((& $PSMUX -L $ns show-options -gv '@n1') -join "`n") "a'b"
    & $PSMUX -L $ns if-shell "exit 0" "set -g @n2 'C:\Users\x\new'"
    Check "if-shell branch 'C:\Users\x\new'" ((& $PSMUX -L $ns show-options -gv '@n2') -join "`n") 'C:\Users\x\new'
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
if ($script:TestsFailed -eq 0) { Remove-Item -Recurse -Force $work -EA SilentlyContinue }
exit $script:TestsFailed
