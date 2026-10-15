# Issue #809: list-sessions printed a server it could not read as a row at exit 0.
#   A) a server that does not answer within the read timeout gave an EMPTY row
#      with -F (and its bare name without -F), stderr empty, rc 0
#   B) a server that rejects the key gave "ERROR: Invalid session key" AS A ROW,
#      stderr empty, rc 0
# Expected: the rows of the servers that answered are printed, the unreadable
# server gets no row, the failure goes to stderr, and the exit code is 1.
#
# Fully isolated: private PSMUX_DATA_DIR and a unique -L namespace. Only the
# server PIDs this script started are suspended, and cleanup is scoped to the
# test namespace.
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "t809_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
$Data = Join-Path $env:TEMP "psmux-809-$PID-$NS"
New-Item -ItemType Directory -Force $Data | Out-Null

# list-windows -a and list-panes -a walk the same registry with their own
# socket and had the same exact string filter.
function CheckAll($phase) {
    foreach ($cmd in @(@('list-windows', '-a', '-F', '#{session_name}'), @('list-panes', '-a', '-F', '#{session_name}'))) {
        $r = Probe $cmd; $tag = "$phase $($cmd[0]) -a"; Show $tag $r
        if (($r.rows -join "`n") -match 'ERROR') { Fail "${tag}: an ERROR reply printed as a row" }
        elseif ($r.rows.Count -ge 1 -and -not ($r.rows | Where-Object { $_ -ne 'alpha' })) { Pass "${tag}: only alpha's rows" }
        else { Fail "${tag}: unexpected rows" }
        if ($r.stderr -match 'beta') { Pass "${tag}: failure named on stderr" } else { Fail "${tag}: stderr does not name beta" }
        if ($r.rc -ne 0) { Pass "${tag}: non-zero exit" } else { Fail "${tag}: exit 0" }
    }
}
$before = (& psmux ls 2>$null) -join "`n"
$oldData = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $Data
$env:PSMUX_NO_WARM = '1'
$script:Pass = 0; $script:Fail = 0
function Pass($m){ Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Fail($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }

Add-Type -Namespace T809 -Name P -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern System.IntPtr OpenProcess(int a, bool i, int pid);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(System.IntPtr h);
[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(System.IntPtr h);
[DllImport("ntdll.dll")] public static extern int NtResumeProcess(System.IntPtr h);
'@ -EA SilentlyContinue

# stdout and stderr go to separate files so they can never be conflated.
function Probe([string[]]$a) {
    $o = [IO.Path]::GetTempFileName(); $e = [IO.Path]::GetTempFileName()
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $PSMUX -ArgumentList (@('-L', $NS) + $a) -NoNewWindow -Wait -PassThru `
         -RedirectStandardOutput $o -RedirectStandardError $e
    $r = @{
        rc = $p.ExitCode; ms = [int]$sw.ElapsedMilliseconds
        rows = @(Get-Content $o -EA SilentlyContinue)
        stderr = "$(Get-Content $e -Raw -EA SilentlyContinue)"
    }
    Remove-Item $o, $e -Force -EA SilentlyContinue
    $r
}
function Show($label, $r) {
    Write-Host ("    {0}: rc={1} {2} ms rows={3} stderr=[{4}]" -f $label, $r.rc, $r.ms,
        (($r.rows | ForEach-Object { "[$_]" }) -join ' '), ($r.stderr -replace "`r?`n", ' ').Trim())
}

$h = [IntPtr]::Zero; $suspended = $false
try {
    & $PSMUX -L $NS new-session -d -s alpha
    & $PSMUX -L $NS new-session -d -s beta
    Start-Sleep -Milliseconds 1500
    $pidFile = Get-ChildItem $Data -Filter '*beta.pid' | Select-Object -First 1
    $keyFile = Get-ChildItem $Data -Filter '*beta.key' | Select-Object -First 1
    if (-not $pidFile -or -not $keyFile) { Write-Host "SETUP FAILED: no beta pid/key in $Data"; Get-ChildItem $Data; exit 1 }
    $pidBeta = [int]((Get-Content $pidFile.FullName -Raw).Split(':')[0].Trim())

    Write-Host "=== Issue #809: list-sessions and an unreadable server ===" -ForegroundColor Cyan

    # CONTROL
    $r = Probe @('list-sessions', '-F', '#{session_name}'); Show 'control -F' $r
    if ($r.rc -eq 0 -and ($r.rows -join ',') -eq 'alpha,beta' -and -not $r.stderr.Trim()) { Pass "control: both rows, rc 0, stderr empty" }
    else { Fail "control invalid" }
    foreach ($cmd in @(@('list-windows', '-a', '-F', '#{session_name}'), @('list-panes', '-a', '-F', '#{session_name}'))) {
        $r = Probe $cmd; Show "control $($cmd[0]) -a" $r
        if ($r.rc -eq 0 -and ($r.rows -join ',') -eq 'alpha,beta' -and $r.ms -lt 1000) { Pass "control $($cmd[0]) -a: both rows, rc 0, fast" }
        else { Fail "control $($cmd[0]) -a invalid" }
    }

    # A) stalled server: suspend beta's server (a PID this script started)
    $h = [T809.P]::OpenProcess(0x0800, $false, $pidBeta)
    if ($h -eq [IntPtr]::Zero) { throw "OpenProcess($pidBeta) failed" }
    if ([T809.P]::NtSuspendProcess($h) -ne 0) { throw "NtSuspendProcess failed" }
    $suspended = $true
    foreach ($fmt in @(@('-F', '#{session_name}'), @())) {
        $tag = if ($fmt.Count) { '-F' } else { 'no -F' }
        $r = Probe (@('list-sessions') + $fmt); Show "stalled $tag" $r
        $alphaOk = $r.rows.Count -ge 1 -and $r.rows[0] -match '^alpha'
        if ($alphaOk -and $r.rows.Count -eq 1) { Pass "stalled ${tag}: only alpha's row printed" }
        else { Fail "stalled ${tag}: expected only alpha's row, got $($r.rows.Count) row(s)" }
        if ($r.stderr -match 'beta') { Pass "stalled ${tag}: failure named on stderr" } else { Fail "stalled ${tag}: stderr does not name beta" }
        if ($r.rc -ne 0) { Pass "stalled ${tag}: non-zero exit" } else { Fail "stalled ${tag}: exit 0" }
        if ($r.ms -lt 2000) { Pass "stalled ${tag}: bounded ($($r.ms) ms)" } else { Fail "stalled ${tag}: took $($r.ms) ms" }
    }
    CheckAll 'stalled'
    [void][T809.P]::NtResumeProcess($h); $suspended = $false
    Start-Sleep -Milliseconds 300

    $r = Probe @('list-sessions', '-F', '#{session_name}'); Show 'resumed -F' $r
    if ($r.rc -eq 0 -and ($r.rows -join ',') -eq 'alpha,beta') { Pass "resumed: both rows again at rc 0" } else { Fail "resumed: not restored" }

    # B) key mismatch: overwrite beta's key file (our own test server)
    $key = Get-Content $keyFile.FullName -Raw
    try {
        Set-Content $keyFile.FullName '0000000000000000' -NoNewline
        foreach ($fmt in @(@('-F', '#{session_name}'), @())) {
            $tag = if ($fmt.Count) { '-F' } else { 'no -F' }
            $r = Probe (@('list-sessions') + $fmt); Show "badkey $tag" $r
            if (($r.rows -join "`n") -match 'ERROR') { Fail "badkey ${tag}: an ERROR reply printed as a row" }
            elseif ($r.rows.Count -eq 1 -and $r.rows[0] -match '^alpha') { Pass "badkey ${tag}: only alpha's row printed" }
            else { Fail "badkey ${tag}: expected only alpha's row, got $($r.rows.Count) row(s)" }
            if ($r.stderr -match 'beta' -and $r.stderr -match 'key') { Pass "badkey ${tag}: failure named on stderr" } else { Fail "badkey ${tag}: stderr does not report it" }
            if ($r.rc -ne 0) { Pass "badkey ${tag}: non-zero exit" } else { Fail "badkey ${tag}: exit 0" }
        }
        CheckAll 'badkey'
    } finally { Set-Content $keyFile.FullName $key -NoNewline }

    $r = Probe @('list-sessions', '-F', '#{session_name}'); Show 'key restored -F' $r
    if ($r.rc -eq 0 -and ($r.rows -join ',') -eq 'alpha,beta' -and -not $r.stderr.Trim()) { Pass "key restored: both rows, rc 0, stderr empty" } else { Fail "key restored: not clean" }
} catch { Fail "script error: $_" } finally {
    if ($suspended) { [void][T809.P]::NtResumeProcess($h) }
    if ($h -ne [IntPtr]::Zero) { [void][T809.P]::CloseHandle($h) }
    & $PSMUX -L $NS kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 800
    $env:PSMUX_DATA_DIR = $oldData
    Remove-Item -Recurse -Force $Data -EA SilentlyContinue
}
$after = (& psmux ls 2>$null) -join "`n"
if ($before -ne $after) { Write-Host "WARNING: default namespace session list changed" -ForegroundColor Yellow }

Write-Host "`nResults: $script:Pass passed, $script:Fail failed"
exit $(if ($script:Fail) { 1 } else { 0 })
