# `psmux set-hook -g after-new-window 'set -ga @a x \; set -ga @b y'` must
# store the whole command list as the hook, as tmux does: an argv element is
# only split when it ENDS in `;` (cmd-parse.y:1094-1103). The CLI joined argv
# with bare spaces, so the server cut the line at the `\;`: the hook held
# `set -ga @a x` and `set -ga @b y` ran at once. The TCP line, control mode and
# in process routes were right already; they are checked here too.
#
# Isolation: PSMUX_DATA_DIR is a fresh temp dir, every command uses -L,
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_EXE picks the binary. Exit code
# is the fail count.

$ErrorActionPreference = "Continue"
$Exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_hkcl_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$ns = "hkcl_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:pass = 0; $script:fail = 0
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { $script:pass++; Write-Host "  [PASS] $name = '$got'" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  [FAIL] $name expected '$want', got '$got'" -ForegroundColor Red }
}
function P { & $Exe -L $ns @args 2>$null }
function Opt($n) { ((P show -gv $n) -join '') }
function Hook { ((P show-hooks -g | Select-String 'after-new-window') -join ' | ') }
# The hook as stored, that nothing ran yet, and what one new-window does.
function Verify([string]$Label, [string]$WantHook) {
    Check "${Label}: stored hook" (Hook) "after-new-window -> $WantHook"
    Check "${Label}: nothing ran at set-hook time" ("a=" + (Opt '@a') + " b=" + (Opt '@b')) "a= b="
    P new-window -t s | Out-Null; Start-Sleep -Milliseconds 800
    Check "${Label}: one new-window runs both" ("a=" + (Opt '@a') + " b=" + (Opt '@b')) "a=x b=y"
    P set-hook -gu after-new-window | Out-Null; P set -gu '@a' | Out-Null; P set -gu '@b' | Out-Null
}
$cc = $null
try {
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2

    Write-Host "`n[1] CLI" -ForegroundColor Yellow
    P set-hook -g after-new-window 'set -ga @a x \; set -ga @b y' | Out-Null
    Verify "CLI \;" 'set -ga @a x \; set -ga @b y'
    P set-hook -g after-new-window 'set -ga @a x ; set -ga @b y' | Out-Null
    Verify "CLI ;" 'set -ga @a x ; set -ga @b y'

    Write-Host "`n[2] raw TCP line" -ForegroundColor Yellow
    $port = (Get-Content (Get-ChildItem $env:PSMUX_DATA_DIR -Filter '*s.port' | Select-Object -First 1).FullName -Raw).Trim()
    $key = (Get-Content (Get-ChildItem $env:PSMUX_DATA_DIR -Filter '*s.key' | Select-Object -First 1).FullName -Raw).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port); $st = $tcp.GetStream(); $tcp.ReceiveTimeout = 3000
    $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("set-hook -g after-new-window `"set -ga @a x \; set -ga @b y`"`n"); $w.Flush(); try { $null = $r.ReadLine() } catch {}
    $tcp.Close()
    Verify "TCP" 'set -ga @a x \; set -ga @b y'

    Write-Host "`n[3] control mode" -ForegroundColor Yellow
    $psi = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in @('-L', $ns, '-C', 'attach', '-t', 's')) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $cc = [Diagnostics.Process]::Start($psi); Start-Sleep -Seconds 1
    $cc.StandardInput.WriteLine('set-hook -g after-new-window "set -ga @a x \; set -ga @b y"'); $cc.StandardInput.Flush(); Start-Sleep -Seconds 1
    $cc.StandardInput.Close(); if (-not $cc.WaitForExit(3000)) { $cc.Kill() }
    Verify "control" 'set -ga @a x ; set -ga @b y'

    Write-Host "`n[4] in process (a hook running set-hook)" -ForegroundColor Yellow
    P set-hook -g after-split-window "set-hook -g after-new-window 'set -ga @a x \; set -ga @b y'" | Out-Null
    P split-window -t s | Out-Null; Start-Sleep -Milliseconds 800
    P set-hook -gu after-split-window | Out-Null
    Verify "in process" 'set -ga @a x \; set -ga @b y'

    Write-Host "`n[5] other CLI set-hook forms unchanged" -ForegroundColor Yellow
    P set-hook -g pane-exited 'display hi' | Out-Null
    P set-hook -ga pane-exited 'display bye' | Out-Null
    Check "-ga appends" ((P show-hooks -g | Select-String 'pane-exited') -join ' | ') "pane-exited[0] -> display hi | pane-exited[1] -> display bye"
    P set-hook -g after-split-window "set -g @p 'C:\a b\c'" | Out-Null
    P split-window -t s | Out-Null; Start-Sleep -Milliseconds 800
    Check "backslash path in the hook" (Opt '@p') 'C:\a b\c'
}
finally {
    if ($cc -and -not $cc.HasExited) { try { $cc.Kill() } catch {} }
    P kill-server | Out-Null
    Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
}
Write-Host "`n  Passed: $script:pass  Failed: $script:fail"
exit $script:fail
