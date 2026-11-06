# `set-hook -g name 'cmd args'` over control mode (and a raw TCP line) stored
# the command WITH its quotes, so the hook ran a command literally named
# `new-window -d -n x` and nothing happened. tmux takes the one argument as
# the command string (cmd-set-option.c, set-hook). The CLI already stored it
# unquoted. Also checks the unquoted multi word form and a quoted argument
# inside the command.
#
# Private -L namespace, cleanup with `psmux -L <ns> kill-server` only.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$utf8 = [System.Text.UTF8Encoding]::new($false)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_shq_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'shq_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
function P { & $PSMUX -L $ns @args 2>&1 }
function Names { return @((P list-windows -F '#{window_name}') | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) }
function Wait-Until([scriptblock]$cond, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return [bool](& $cond)
}
function Read-Shared([string]$path) {
    try {
        $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
        $ms = [System.IO.MemoryStream]::new(); $fs.CopyTo($ms); $fs.Close()
        return , $ms.ToArray()
    } catch { return , [byte[]]@() }
}
function Get-Lines([string]$path) { return @($utf8.GetString((Read-Shared $path)) -split "`n" | ForEach-Object { $_.TrimEnd("`r") }) }
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
$out = Join-Path $work 'cc.out'
$p = [System.Diagnostics.Process]::Start($psi)
$fo = [System.IO.FileStream]::new($out, 'Create', 'Write', 'ReadWrite', 1)
$t1 = $p.StandardOutput.BaseStream.CopyToAsync($fo, 1)
function Invoke-CC([string]$line) {
    $before = (Get-Blocks $out).Count
    $p.StandardInput.Write("$line`n"); $p.StandardInput.Flush()
    Wait-Until { (Get-Blocks $out).Count -ge $before + 1 } 7000 | Out-Null
    return (Get-Blocks $out)[-1]
}
function Send-Tcp([string]$cmd) {
    $dir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE '.psmux' }
    $port = (Get-Content (Join-Path $dir "${ns}__demo.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $dir "${ns}__demo.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    $s = $tcp.GetStream(); $s.ReadTimeout = 3000
    $w = [System.IO.StreamWriter]::new($s); $rd = [System.IO.StreamReader]::new($s)
    $w.Write("AUTH $key`n$cmd`n"); $w.Flush(); $null = $rd.ReadLine()
    try { $null = $rd.ReadToEnd() } catch {}
    $tcp.Close()
}
# Each check splits the same 80x24 window.  A split that does not fit is
# refused and, as in tmux, fires no after-split-window (#1000), so keep the
# window at one pane and let every check's split really happen.
function Split-Fresh {
    P kill-pane -a -t demo | Out-Null
    P split-window -t demo | Out-Null
}
function Hook-Line([string]$name) { return ((P show-hooks -g) | Where-Object { "$_" -match "^$name " } | Select-Object -First 1) }
function Check-Hook([string]$label, [string]$expectWin) {
    $hl = "$(Hook-Line 'after-split-window')"
    if ($hl -match "-> new-window -d -n $expectWin$") { Write-Pass "$label stored '$hl'" } else { Write-Fail "$label stored '$hl'" }
    Split-Fresh
    if (Wait-Until { (Names) -contains $expectWin } 4000) { Write-Pass "$label hook ran (window $expectWin)" } else { Write-Fail "$label hook did not run: windows $((Names) -join ',')" }
    P set-hook -gu after-split-window | Out-Null
}

try {
    if (-not (Wait-Until { (Get-Lines $out | Where-Object { $_ -match '^%session-changed' }).Count -gt 0 } 10000)) { Write-Fail "-CC session did not start"; throw 'no session' }

    Write-Host "`n[1] control mode, one quoted command argument" -ForegroundColor Yellow
    $b = Invoke-CC "set-hook -g after-split-window 'new-window -d -n hookcc'"
    if ($b.End -eq 'end') { Write-Pass "set-hook -> %end" } else { Write-Fail "set-hook -> $($b.End) $($b.Body -join '|')" }
    Check-Hook 'control mode quoted' 'hookcc'

    Write-Host "`n[2] control mode, the command as separate words" -ForegroundColor Yellow
    Invoke-CC "set-hook -g after-split-window new-window -d -n hookwords" | Out-Null
    Check-Hook 'control mode words' 'hookwords'

    Write-Host "`n[3] raw TCP line, one quoted command argument" -ForegroundColor Yellow
    Send-Tcp "set-hook -g after-split-window 'new-window -d -n hooktcp'"
    Check-Hook 'TCP quoted' 'hooktcp'

    Write-Host "`n[4] CLI, one command argument" -ForegroundColor Yellow
    P set-hook -g after-split-window 'new-window -d -n hookcli' | Out-Null
    Check-Hook 'CLI' 'hookcli'

    Write-Host "`n[5] a quoted argument inside the command survives" -ForegroundColor Yellow
    Invoke-CC "set-hook -g after-split-window `"rename-window 'two words'`"" | Out-Null
    $hl = "$(Hook-Line 'after-split-window')"
    Split-Fresh
    if (Wait-Until { (Names) -contains 'two words' } 4000) { Write-Pass "hook renamed the window to 'two words' ($hl)" } else { Write-Fail "hook with a quoted argument: '$hl', windows $((Names) -join ',')" }
    P set-hook -gu after-split-window | Out-Null
} catch {
    if ("$_" -ne 'no session') { Write-Fail "exception: $_" }
} finally {
    P kill-server | Out-Null
    if (-not $p.WaitForExit(3000)) { try { $p.Kill() } catch {} }
    try { $t1.Wait(2000) | Out-Null } catch {}
    $fo.Close()
    if ($script:TestsFailed -gt 0) { Write-Host "`n-CC transcript: $out" }
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { Remove-Item -Recurse -Force $work -EA SilentlyContinue; exit 0 }
