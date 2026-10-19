# A control client (`-C attach`) can run new-session on its stdin, as in tmux
# (cmd-new-session.c runs for any client). Compared with tmux 3.8 in WSL:
#   new-session -d -s K     -> %begin / %end, then %sessions-changed; K exists
#   new-session -d -s base  -> "duplicate session: base" inside %error
#   new-session -d -P -F .. -> the report line inside %begin / %end
# psmux used to answer `unknown command: new-session` inside %error.
#
# Runs only under -L in a private PSMUX_DATA_DIR; no window is opened.

$ErrorActionPreference = 'Continue'
$Exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_test_control_new_session_$PID"
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$script:pass = 0
$script:fail = 0
function Check([string]$name, [bool]$ok, [string]$detail) {
    if ($ok) { $script:pass++; Write-Host "PASS $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "FAIL $name : $detail" -ForegroundColor Red }
}

# Run one control client attached to `base`, feed it $cmds one at a time and
# return its stream without %output lines, with each block reduced to
# "%begin" / "%end" / "%error" so the order can be compared.
function Invoke-Control([string]$ns, [string[]]$cmds) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = "-L $ns -C attach -t base"
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 800
    foreach ($c in $cmds) {
        $p.StandardInput.WriteLine($c)
        $p.StandardInput.Flush()
        Start-Sleep -Milliseconds 1500
    }
    $p.StandardInput.Close()
    if (-not $p.WaitForExit(8000)) { $p.Kill() }
    $text = $outTask.Result
    $text -split "`r?`n" | Where-Object { $_ -and $_ -notmatch '^%output' } |
        ForEach-Object { $_ -replace '^(%begin|%end|%error) .*$', '$1' }
}

# The lines of the block a command produced: the Nth %begin (1 based, the
# initial attach block is 0) up to and including its %end or %error, plus the
# line right after it.
function Get-Block($lines, [int]$n) {
    $seen = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -eq '%begin') {
            $seen++
            if ($seen -eq $n) {
                $out = @()
                for ($j = $i + 1; $j -lt $lines.Count; $j++) {
                    $out += $lines[$j]
                    if ($lines[$j] -in '%end', '%error') {
                        if ($j + 1 -lt $lines.Count) { $out += $lines[$j + 1] }
                        return , $out
                    }
                }
                return , $out
            }
        }
    }
    return , @()
}

$ns = "test_control_new_session_${PID}_$(Get-Random)"
& $Exe -L $ns new -d -s base | Out-Null
try {
    $lines = @(Invoke-Control $ns @(
        'new-session -d -s K',
        'new-session -d -s base',
        'new-session -d -P -F "#{session_name}" -s PP',
        'display-message -p "#{session_name}"'
    ))
    $all = $lines -join ' | '

    $b1 = Get-Block $lines 1
    Check 'new-session -d -s K is not an unknown command' (-not ($all -match 'unknown command')) $all
    Check 'new-session -d -s K ends with %end and no output' ($b1.Count -ge 1 -and $b1[0] -eq '%end') ($b1 -join ' | ')
    Check 'new-session -d -s K is followed by %sessions-changed' ($b1.Count -ge 2 -and $b1[1] -eq '%sessions-changed') ($b1 -join ' | ')
    & $Exe -L $ns has-session -t K 2>$null
    Check 'session K exists afterwards' ($LASTEXITCODE -eq 0) "has-session rc=$LASTEXITCODE"

    $b2 = Get-Block $lines 2
    Check 'duplicate name is the %error "duplicate session: base"' ($b2.Count -ge 2 -and $b2[0] -eq 'duplicate session: base' -and $b2[1] -eq '%error') ($b2 -join ' | ')

    $b3 = Get-Block $lines 3
    Check '-P -F prints the new session name' ($b3.Count -ge 2 -and $b3[0] -eq 'PP' -and $b3[1] -eq '%end') ($b3 -join ' | ')
    & $Exe -L $ns has-session -t PP 2>$null
    Check 'session PP exists afterwards' ($LASTEXITCODE -eq 0) "has-session rc=$LASTEXITCODE"

    $b4 = Get-Block $lines 4
    Check 'the control client stays on base (-d)' ($b4.Count -ge 1 -and $b4[0] -eq 'base') ($b4 -join ' | ')

    $names = @(& $Exe -L $ns ls -F '#{session_name}') | Sort-Object
    Check 'list-sessions shows base, K and PP' (($names -join ',') -eq 'base,K,PP') ($names -join ',')
} finally {
    & $Exe -L $ns kill-server 2>$null | Out-Null
}

Write-Host ""
Write-Host "Passed: $script:pass  Failed: $script:fail"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }
