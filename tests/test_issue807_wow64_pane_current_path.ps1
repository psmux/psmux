# Issue #807: #{pane_current_path} must report the cwd of a 32-bit (WOW64)
# foreground process, not C:\Windows.
#
# A pane runs a native pwsh/cmd shell, cd's into a temp dir, then launches
# C:\Windows\SysWOW64\cmd.exe (an x86 image, like every scoop shim). The pane's
# foreground process is now the WOW64 cmd.exe and pane_current_path must still
# be the temp dir. A native System32 cmd.exe is checked the same way as a
# control, and split-window -c "#{pane_current_path}" must open in the temp dir.
#
# Isolated: unique -L namespace, PSMUX_TEST_BIN honoured, cleanup by namespace.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux).Source }
$NS = "i807_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
$S = "wow"
$pass = 0; $fail = 0
function Ok($m) { $script:pass++; Write-Host "  [PASS] $m" -ForegroundColor Green }
function Bad($m) { $script:fail++; Write-Host "  [FAIL] $m" -ForegroundColor Red }

$wow = Join-Path $env:SystemRoot 'SysWOW64\cmd.exe'
$nat = Join-Path $env:SystemRoot 'System32\cmd.exe'
if (-not (Test-Path $wow)) { Write-Host "SKIP: no SysWOW64 on this machine"; exit 0 }

$dir = Join-Path $env:TEMP ("psmux_i807_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $dir -Force | Out-Null
$dir = (Get-Item $dir).FullName

function Norm($p) { if ($null -eq $p) { return '' } ($p.Trim().TrimEnd('\')).ToLowerInvariant() }

function Wait-Path($target, $want, $timeoutMs = 8000) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $got = ''
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $got = (& $PSMUX -L $NS display-message -p -t $target '#{pane_current_path}' 2>&1 | Out-String).Trim()
        if ((Norm $got) -eq (Norm $want)) { return $got }
        Start-Sleep -Milliseconds 250
    }
    return $got
}

function Wait-Cmd($target, $exeName, $timeoutMs = 8000) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $got = ''
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $got = (& $PSMUX -L $NS display-message -p -t $target '#{pane_current_command}' 2>&1 | Out-String).Trim()
        if ($got -like "$exeName*") { return $got }
        Start-Sleep -Milliseconds 250
    }
    return $got
}

Write-Host "=== Issue #807: WOW64 pane_current_path (bin: $PSMUX, ns: $NS) ==="
try {
    & $PSMUX -L $NS new-session -d -s $S -x 120 -y 30 -c $dir "cmd.exe /d /k"
    Start-Sleep -Milliseconds 1500

    # Baseline: the shell itself.
    $got = Wait-Path "${S}:0.0" $dir
    if ((Norm $got) -eq (Norm $dir)) { Ok "shell pane_current_path = $got" } else { Bad "shell pane_current_path = '$got', want '$dir'" }

    # Launch a 32-bit cmd.exe as the foreground process.
    & $PSMUX -L $NS send-keys -t "${S}:0.0" "`"$wow`" /d /k" Enter
    $cmdname = Wait-Cmd "${S}:0.0" 'cmd'
    Start-Sleep -Milliseconds 1500
    $got = Wait-Path "${S}:0.0" $dir 5000
    Write-Host "  foreground cmd: $cmdname"
    Write-Host "  WOW64 foreground pane_current_path = '$got'"
    if ((Norm $got) -eq (Norm $dir)) { Ok "WOW64 foreground reports its cwd" } else { Bad "WOW64 foreground pane_current_path = '$got', want '$dir'" }

    # cd inside the WOW64 child to a subdir: must follow.
    $sub = Join-Path $dir 'sub'
    New-Item -ItemType Directory -Path $sub -Force | Out-Null
    & $PSMUX -L $NS send-keys -t "${S}:0.0" "cd /d `"$sub`"" Enter
    $got = Wait-Path "${S}:0.0" $sub 5000
    if ((Norm $got) -eq (Norm $sub)) { Ok "WOW64 cd followed: $got" } else { Bad "after cd in WOW64 child pane_current_path = '$got', want '$sub'" }

    # split-window -c "#{pane_current_path}" must open in the subdir.
    & $PSMUX -L $NS split-window -t "${S}:0.0" -c '#{pane_current_path}' "cmd.exe /d /k"
    Start-Sleep -Milliseconds 1500
    $got = Wait-Path "${S}:0.1" $sub 5000
    if ((Norm $got) -eq (Norm $sub)) { Ok "split-window -c opened in $got" } else { Bad "split-window -c opened in '$got', want '$sub'" }

    # Control: native 64-bit cmd.exe in a second window.
    & $PSMUX -L $NS new-window -t $S -c $dir "cmd.exe /d /k"
    Start-Sleep -Milliseconds 1200
    & $PSMUX -L $NS send-keys -t "${S}:1.0" "`"$nat`" /d /k" Enter
    Start-Sleep -Milliseconds 1500
    $got = Wait-Path "${S}:1.0" $dir 5000
    if ((Norm $got) -eq (Norm $dir)) { Ok "native foreground reports its cwd" } else { Bad "native foreground pane_current_path = '$got', want '$dir'" }
}
finally {
    & $PSMUX -L $NS kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $pass passed, $fail failed"
if ($fail -gt 0) { exit 1 } else { exit 0 }
