# Control mode (-C) parity: show-options -g scope and refresh-client -C sizes.
#
# A control client's `show -gv prefix` answered with the SESSION value (C-a)
# where the CLI and tmux answer with the global one (C-b), `show -g prefix`
# printed a bare value instead of `prefix C-b`, and `refresh-client -C WxH`
# never changed #{client_width}/#{client_height}.
#
# Isolated: a unique -L namespace and a private PSMUX_DATA_DIR, cleaned up
# only with `-L <ns> kill-server`. Pass -Exe to test a specific binary.

param([string]$Exe = $env:PSMUX_TEST_EXE)
$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
if (-not $Exe) { $Exe = (Get-Command psmux -EA Stop).Source }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ($got -eq $want) { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$ns = "ctrlparity$(Get-Random)"
$data = Join-Path $env:TEMP "psmux_$ns"
New-Item -ItemType Directory $data -Force | Out-Null
$savedData = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $data
$p = $null

try {
    & $Exe -L $ns new-session -d -s t -x 80 -y 24
    Start-Sleep -Milliseconds 1500
    & $Exe -L $ns has-session -t t 2>$null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }
    & $Exe -L $ns set -g prefix C-b
    & $Exe -L $ns set -t t prefix C-a
    & $Exe -L $ns set -g status-left GLOBAL_SL
    & $Exe -L $ns set -t t status-left SESSION_SL

    Write-Host "`n=== CLI reference ===" -ForegroundColor Cyan
    Check "CLI show -gv prefix" (& $Exe -L $ns show-options -gv prefix) "C-b"
    Check "CLI show -g prefix" (& $Exe -L $ns show-options -g prefix) "prefix C-b"

    $psi = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in @("-L", $ns, "-C", "attach", "-t", "t")) { $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $p = [Diagnostics.Process]::Start($psi)
    $sb = [Text.StringBuilder]::new()
    # Lines are read here, in order. Register-ObjectEvent delivery can hand a
    # block's body over after its %end, which reads as an empty reply.
    $script:pending = $null
    function Next-Line([int]$ms) {
        if (-not $script:pending) { $script:pending = $p.StandardOutput.ReadLineAsync() }
        if (-not $script:pending.Wait($ms)) { return $null }
        $l = $script:pending.Result
        $script:pending = $null
        if ($null -ne $l) { [void]$sb.AppendLine($l) }
        return $l
    }

    # The body of the next `%begin` ... `%end`/`%error` block. The attach
    # itself answers one empty block first, read before any question.
    function Next-Block([int]$timeoutMs) {
        $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
        $body = $null
        while ([DateTime]::UtcNow -lt $deadline) {
            $l = Next-Line 200
            if ($null -eq $l) { continue }
            if ($null -eq $body) {
                if ($l -match '^%begin ') { $body = [System.Collections.Generic.List[string]]::new() }
                continue
            }
            if ($l -match '^%(end|error) ') { return ($body -join "`n") }
            if ($l -notmatch '^%' -and $l -ne '') { $body.Add($l) }
        }
        return "<timeout>"
    }
    [void](Next-Block 10000)

    function Ask([string]$line) {
        $p.StandardInput.WriteLine($line); $p.StandardInput.Flush()
        return Next-Block 5000
    }

    Write-Host "`n=== show-options scope on the control route ===" -ForegroundColor Cyan
    Check "-C show -gv prefix" (Ask 'show-options -gv prefix') "C-b"
    Check "-C show -g prefix" (Ask 'show-options -g prefix') "prefix C-b"
    Check "-C show -gv status-left" (Ask 'show-options -gv status-left') "GLOBAL_SL"
    Check "-C show -v prefix (session)" (Ask 'show-options -v prefix') "C-a"
    Check "-C show prefix (session)" (Ask 'show-options prefix') "prefix C-a"

    Write-Host "`n=== refresh-client -C sets the client's size ===" -ForegroundColor Cyan
    [void](Ask 'refresh-client -C 50x20')
    Check "after -C 50x20" (Ask 'display -p "#{client_width}x#{client_height}"') "50x20"
    [void](Ask 'refresh-client -C 90x25')
    Check "after -C 90x25" (Ask 'display -p "#{client_width}x#{client_height}"') "90x25"
    Check "window follows" (Ask 'display -p "#{window_width}x#{window_height}"') "90x25"
    Check "list-clients follows" (Ask 'list-clients -F "#{client_width}x#{client_height}"') "90x25"

    Write-Host "`n=== Win32 TUI: an attached window still works ===" -ForegroundColor Cyan
    $tui = Start-Process -FilePath $Exe -ArgumentList "-L", $ns, "attach", "-t", "t" -PassThru
    Start-Sleep -Seconds 3
    $clients = (& $Exe -L $ns list-clients -F '#{client_control_mode}') -join ','
    if ($clients -match '0') { Write-Pass "TUI client attached beside the control client ($clients)" }
    else { Write-Fail "TUI client not listed ($clients)" }
    Check "CLI show -gv prefix with TUI attached" (& $Exe -L $ns show-options -gv prefix) "C-b"
    try { Stop-Process -Id $tui.Id -Force -EA SilentlyContinue } catch {}
}
finally {
    if ($p) {
        try { $p.StandardInput.Close() } catch {}
        if (-not $p.WaitForExit(3000)) { try { $p.Kill() } catch {} }
    }
    & $Exe -L $ns kill-server 2>$null
    $env:PSMUX_DATA_DIR = $savedData
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
