# show-hooks and list-commands print what tmux 3.8 prints.
#
# show-hooks (cmd-show-options.c): nothing when no hook is set on the
# session, -g lists every session hook in table order (bare names when
# empty, `name[i] command` rows when set), -gw the window and pane hooks, a
# named hook its rows (or its bare name under -g). psmux printed
# "(no hooks)", ignored the flags and the name, and used `name -> command`.
#
# list-commands (cmd-list-commands.c): `name (alias) usage` rows in tmux's
# command order, -F with #{command_list_name}, #{command_list_alias} and
# #{command_list_usage}, a command argument found as cmd_find finds it.
# psmux printed a help style list and ignored -F and the argument.
param([string]$Exe)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

if (-not $Exe) {
    $cand = Join-Path $PSScriptRoot '..\target\release\psmux.exe'
    $Exe = if (Test-Path $cand) { (Resolve-Path $cand).Path } else { (Get-Command psmux -ErrorAction Stop).Source }
}
$ns = "tshlc" + (Get-Random -Maximum 999999)
$dataDir = Join-Path $env:TEMP ("psmux_tshlc_" + (Get-Random))
New-Item -ItemType Directory -Force $dataDir | Out-Null
$env:PSMUX_DATA_DIR = $dataDir

$pass = 0; $fail = 0
function Check([bool]$ok, [string]$name, [string]$detail = '') {
    if ($ok) { $script:pass++; Write-Host "  PASS $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  FAIL $name $detail" -ForegroundColor Red }
}
# Raw stdout, stderr and exit code of one psmux run.
function Run([string[]]$a) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    foreach ($x in (@('-L', $ns) + $a)) { [void]$psi.ArgumentList.Add($x) }
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEnd(); $err = $p.StandardError.ReadToEnd(); $p.WaitForExit()
    [pscustomobject]@{ Out = ($out -replace "`r", ''); Err = $err.Trim(); Rc = $p.ExitCode }
}
function Lines($text) { @($text -split "`n" | Where-Object { $_ -ne '' }) }
function RawTcp([string]$line) {
    $pf = Get-ChildItem $dataDir -Recurse -Filter "${ns}__a.port" | Select-Object -First 1
    $kf = Get-ChildItem $dataDir -Recurse -Filter "${ns}__a.key" | Select-Object -First 1
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect('127.0.0.1', [int](Get-Content $pf.FullName -Raw).Trim())
    $s = $tcp.GetStream(); $s.ReadTimeout = 5000
    $req = [System.Text.Encoding]::UTF8.GetBytes("AUTH $((Get-Content $kf.FullName -Raw).Trim())`n$line`n")
    $s.Write($req, 0, $req.Length); $s.Flush()
    $ms = New-Object System.IO.MemoryStream; $buf = New-Object byte[] 65536
    try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $tcp.Close()
    $t = [System.Text.Encoding]::UTF8.GetString($ms.ToArray())
    if ($t.StartsWith("OK`n")) { $t.Substring(3) } else { $t }
}

try {
    $null = Run @('new-session', '-d', '-s', 'a', '-x', '100', '-y', '20')
    Start-Sleep -Milliseconds 1500

    Write-Host "show-hooks with no hooks set"
    $r = Run @('show-hooks'); Check ($r.Out -eq '' -and $r.Rc -eq 0) "show-hooks prints nothing" "[$($r.Out)]"
    Check ((Run @('show-hooks', '-w')).Out -eq '') "show-hooks -w prints nothing"
    Check ((Run @('show-hooks', '-p')).Out -eq '') "show-hooks -p prints nothing"
    $g = Lines (Run @('show-hooks', '-g')).Out
    Check ($g.Count -eq 61) "show-hooks -g lists the 61 session hooks" "got $($g.Count)"
    Check ($g[0] -eq 'after-bind-key' -and $g[-1] -eq 'window-unlinked') "in tmux table order" "$($g[0]) .. $($g[-1])"
    Check (-not ($g -match 'no hooks')) "no (no hooks) line"
    Check (-not ($g -contains 'pane-exited')) "pane hooks are not session hooks"
    $gw = Lines (Run @('show-hooks', '-gw')).Out
    Check ($gw.Count -eq 27 -and ($gw -contains 'pane-exited') -and ($gw -contains 'window-renamed')) "show-hooks -gw lists the 27 window and pane hooks" "got $($gw.Count)"
    Check ((Run @('show-hooks', '-g', 'session-created')).Out -eq "session-created`n") "an empty named hook prints its bare name under -g"
    Check ((Run @('show-hooks', 'session-created')).Out -eq '') "an empty named hook prints nothing without -g"

    Write-Host "show-hooks with hooks set"
    $null = Run @('set-hook', '-g', 'after-new-window', 'display-message hi')
    $null = Run @('set-hook', '-g', 'pane-exited', 'display-message one')
    $null = Run @('set-hook', '-ga', 'pane-exited', 'display-message two')
    $g = Lines (Run @('show-hooks', '-g')).Out
    Check ($g -contains 'after-new-window[0] display-message hi') "a set hook prints name[0] command" ($g | Select-String 'after-new-window')
    Check (-not ($g -contains 'after-new-window')) "and no bare name for it"
    $i1 = [array]::IndexOf($g, 'after-list-windows'); $i2 = [array]::IndexOf($g, 'after-new-window[0] display-message hi'); $i3 = [array]::IndexOf($g, 'after-paste-buffer')
    Check ($i1 -lt $i2 -and $i2 -lt $i3) "set hook stays in table order" "$i1 $i2 $i3"
    Check (-not ($g -match '->')) "no -> rows"
    $gw = (Run @('show-hooks', '-gw')).Out
    Check ($gw.Contains("pane-exited[0] display-message one`npane-exited[1] display-message two`n")) "pane-exited rows under -gw"
    Check ((Run @('show-hooks')).Out -eq "after-new-window[0] display-message hi`n") "show-hooks lists only the set session hook"
    Check ((Run @('show-hooks', '-w')).Out -eq "pane-exited[0] display-message one`npane-exited[1] display-message two`n") "show-hooks -w lists only the set pane hook"
    Check ((Run @('show-hooks', '-g', 'pane-exited[1]')).Out -eq "pane-exited[1] display-message two`n") "an indexed name prints that element"
    $null = Run @('set-hook', '-gu', 'after-new-window')
    Check ((Run @('show-hooks', '-g', 'after-new-window')).Out -eq "after-new-window`n") "after -gu the hook is empty again (bare name under -g)"
    Check ((Run @('show-hooks')).Out -eq '') "and show-hooks prints nothing again"
    $r = Run @('show-hooks', '-g', 'no-such-hook')
    Check ($r.Rc -eq 1 -and $r.Err -match 'invalid option: no-such-hook') "an unknown hook name is still an error" "rc=$($r.Rc) err=$($r.Err)"
    Check ((RawTcp 'show-hooks -w') -eq "pane-exited[0] display-message one`npane-exited[1] display-message two`n") "raw TCP show-hooks -w"
    Check ((RawTcp 'show-hooks') -eq '') "raw TCP show-hooks with nothing set replies nothing"

    Write-Host "list-commands"
    $r = Run @('list-commands'); $l = Lines $r.Out
    Check ($l.Count -eq 92 -and $r.Rc -eq 0) "92 rows" "got $($l.Count)"
    Check ($l[0] -like 'attach-session (attach) *' -and $l[-1] -like 'wait-for (wait) *') "tmux command order with aliases" "$($l[0]) .. $($l[-1])"
    Check ($l -contains 'list-commands (lscm) [-F format] [command]') "tmux usage strings"
    Check ($l -contains 'kill-server ') "a command without alias or usage keeps the template space"
    Check ($r.Out.EndsWith("`n") -and -not $r.Out.EndsWith("`n`n")) "ends in one newline"
    Check ((Run @('list-commands', 'neww')).Out -like "new-window (neww) [[]-abdEkPS]*") "a command by alias"
    Check ((Run @('lscm', 'show-h')).Out -eq "show-hooks [-Bgpw] [-t target-pane] [hook]`n") "a command by unique prefix"
    $r = Run @('list-commands', 'bogus'); Check ($r.Rc -eq 1 -and $r.Err -match 'unknown command: bogus' -and $r.Out -eq '') "unknown command is an error" "rc=$($r.Rc) err=$($r.Err)"
    $r = Run @('list-commands', 'list-'); Check ($r.Rc -eq 1 -and $r.Err -match 'ambiguous command: list-, could be: list-buffers') "ambiguous command is an error" "rc=$($r.Rc) err=$($r.Err)"
    $r = Run @('list-commands', '-F', '#{command_list_name}'); $l = Lines $r.Out
    Check ($l.Count -eq 92 -and $l[0] -eq 'attach-session' -and ($l -contains 'show-hooks')) "-F #{command_list_name}" "count $($l.Count) first $($l[0])"
    $r = Run @('list-commands', '-F', '#{command_list_name}|#{command_list_alias}|#{command_list_usage}', 'lscm')
    Check ($r.Out -eq "list-commands|lscm|[-F format] [command]`n") "-F with all three variables and a command" "[$($r.Out)]"
    $r = Run @('list-commands', '-F', '#{?command_list_alias,#{command_list_name},}'); $l = Lines $r.Out
    Check ($l.Count -gt 50 -and $l.Count -lt 92 -and -not ($l -contains 'kill-server')) "rows that expand to nothing are left out" "count $($l.Count)"
    $r = Run @('list-commands', '-F', '#{=4:command_list_name}', 'attach'); Check ($r.Out -eq "atta`n") "format modifiers work (#{=4:...})" "[$($r.Out)]"
    $t = RawTcp 'list-commands -F "#{command_list_name}" kill-server'
    Check ($t -eq "kill-server`n") "raw TCP list-commands -F" "[$t]"
    Check ((RawTcp 'list-commands bogus') -match '^ERROR: unknown command: bogus') "raw TCP unknown command is an ERROR reply"
}
finally {
    $null = & $Exe -L $ns kill-server 2>$null
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Passed: $pass  Failed: $fail"
if ($fail -gt 0) { exit 1 } else { exit 0 }
