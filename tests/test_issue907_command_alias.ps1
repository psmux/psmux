# Issue #907: command-alias on every route. tmux's own spelling `set -s
# command-alias[100] 'x=...'` was kept as plain text, so the alias never
# existed; `psmux x` was refused by the client before the server saw it; the
# server split an alias's command on whitespace, so `rename-window "Q Q"`
# named the window `"Q`, and an alias with no command panicked the
# connection thread.
#
# Isolated: USERPROFILE, PSMUX_DATA_DIR and PSMUX_CONFIG_FILE point into a
# fresh temp dir and every command uses -L. PSMUX_EXE picks the binary
# (defaults to psmux on PATH).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$TempHome = Join-Path $env:TEMP ("psmux_alias_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "empty.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_TARGET*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$NS = "alias_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function P { & $PSMUX -L $NS @args 2>&1 }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}
function WindowName { (P display -p '#{window_name}') -join '' }

try {
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t s | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }
    P set -g allow-rename off | Out-Null

    # tmux 3.7c on a new server (options-table.c:301-306).
    $defaults = @(
        'command-alias[0] split-pane=split-window',
        'command-alias[1] splitp=split-window',
        'command-alias[2] "server-info=show-messages -JT"',
        'command-alias[3] "info=show-messages -JT"',
        'command-alias[4] "choose-window=choose-tree -w"',
        'command-alias[5] "choose-session=choose-tree -s"'
    )

    Write-Host "`n[0] a new server holds tmux's default aliases" -ForegroundColor Yellow
    Check "show -s command-alias" ((P show -s command-alias) -join ' | ') ($defaults -join ' | ')

    Write-Host "`n[1] command-alias[N] is an element of the array" -ForegroundColor Yellow
    P set -s 'command-alias[100]' 'zz=rename-window ZZ' | Out-Null
    Check "show -s command-alias" ((P show -s command-alias) -join ' | ') (($defaults + 'command-alias[100] "zz=rename-window ZZ"') -join ' | ')
    Check "show -sv command-alias[100]" ((P show -sv 'command-alias[100]') -join '') 'zz=rename-window ZZ'

    Write-Host "`n[2] the CLI runs an alias" -ForegroundColor Yellow
    P zz | Out-Null
    Check "psmux zz exit status" $LASTEXITCODE 0
    Check "window name after psmux zz" (WindowName) "ZZ"

    Write-Host "`n[3] quotes in the alias and in the caller's words survive" -ForegroundColor Yellow
    P set -s 'command-alias[101]' 'qq=rename-window "Q Q"' | Out-Null
    P qq | Out-Null
    Check "window name after psmux qq" (WindowName) "Q Q"
    P set -s 'command-alias[102]' 'nm=rename-window' | Out-Null
    P nm "A B" | Out-Null
    Check "window name after psmux nm 'A B'" (WindowName) "A B"

    Write-Host "`n[4] an alias holding a command list; the caller's words go to its last command" -ForegroundColor Yellow
    P set -s 'command-alias[103]' 'two=set -g @a 1 ; set -g @b' | Out-Null
    P two 2 | Out-Null
    Check "@a" ((P show -gv '@a') -join '') "1"
    Check "@b" ((P show -gv '@b') -join '') "2"

    Write-Host "`n[5] an alias of a command whose output the client prints" -ForegroundColor Yellow
    P set -s 'command-alias[104]' 'wn=display-message -p' | Out-Null
    Check "psmux wn '#{session_name}'" ((P wn '#{session_name}') -join '') "s"

    Write-Host "`n[6] a hook runs an alias" -ForegroundColor Yellow
    P rename-window base | Out-Null
    P set-hook -g after-split-window 'qq' | Out-Null
    P split-window -d -t s | Out-Null
    Start-Sleep -Milliseconds 800
    Check "window name after the hook" (WindowName) "Q Q"
    P set-hook -gu after-split-window | Out-Null

    Write-Host "`n[7] a CLI command list runs an alias on the server" -ForegroundColor Yellow
    P rename-window base | Out-Null
    Check "qq ; display" ((P qq `; display -p '#{window_name}') -join '') "Q Q"

    Write-Host "`n[8] an alias with no command runs nothing and the server lives" -ForegroundColor Yellow
    P set -s 'command-alias[105]' 'e=' | Out-Null
    Check "e ; display -p ok" ((P e `; display -p ok) -join '') "ok"
    P has-session -t s | Out-Null
    Check "server alive" $LASTEXITCODE 0

    Write-Host "`n[9] source-file runs an alias line" -ForegroundColor Yellow
    $conf = Join-Path $TempHome "alias.conf"
    Set-Content -Path $conf -Value @("set -s command-alias[106] 'sf=set -g @sf yes'", "sf")
    P source-file $conf | Out-Null
    Start-Sleep -Milliseconds 500
    Check "@sf" ((P show -gv '@sf') -join '') "yes"

    Write-Host "`n[10] a control client runs an alias" -ForegroundColor Yellow
    P rename-window base | Out-Null
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $PSMUX
    $psi.Arguments = "-L $NS -C attach -t s"
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $cc = [System.Diagnostics.Process]::Start($psi)
    $outTask = $cc.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 800
    $cc.StandardInput.WriteLine('nm "C C"')
    $cc.StandardInput.WriteLine('wn "#{window_name}"')
    $cc.StandardInput.Flush()
    Start-Sleep -Milliseconds 1500
    $cc.StandardInput.Close()
    if (-not $cc.WaitForExit(8000)) { $cc.Kill() }
    $lines = @($outTask.Result -split "`r?`n" | Where-Object { $_ -and $_ -notmatch '^%' })
    Check "control client output" ($lines -join ' | ') "C C"

    Write-Host "`n[11] -u removes an element; the name is unknown again" -ForegroundColor Yellow
    P set -su 'command-alias[100]' | Out-Null
    $out = (P zz) -join ' '
    $rc = $LASTEXITCODE
    if ($rc -ne 0 -and $out -match 'unknown command: zz') { Write-Pass "psmux zz refused (rc=$rc)" } else { Write-Fail "psmux zz rc=$rc out='$out'" }
    if (((P show -s command-alias) -join ' ') -notmatch 'command-alias\[100\]') { Write-Pass "element 100 gone" } else { Write-Fail "element 100 still listed" }

    Write-Host "`n[13] -u without an index puts the defaults back" -ForegroundColor Yellow
    P set -s command-alias 'x=list-keys' | Out-Null
    Check "plain set replaces the array" ((P show -s command-alias) -join ' | ') 'command-alias[0] x=list-keys'
    P set -su command-alias | Out-Null
    Check "show -s command-alias after -u" ((P show -s command-alias) -join ' | ') ($defaults -join ' | ')

    Write-Host "`n[12] a name that is no alias is still unknown" -ForegroundColor Yellow
    $out = (P nosuchcmd) -join ' '
    $rc = $LASTEXITCODE
    if ($rc -ne 0 -and $out -match 'unknown command: nosuchcmd') { Write-Pass "nosuchcmd refused (rc=$rc)" } else { Write-Fail "nosuchcmd rc=$rc out='$out'" }
}
finally {
    P kill-server | Out-Null
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
