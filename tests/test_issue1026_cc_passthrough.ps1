# Issue #1026: a tmux DCS passthrough written by a pane reached `%output`
# without its closing ESC \. The inbox conhost's ConPTY hands psmux
# `ESC P tmux; ESC ESC ]52;...BEL` and drops the outer ST, so a control
# client (iTerm2's tmux integration) stayed inside a DCS and printed the
# following redraw as text. psmux now unwraps the passthrough for control
# clients: the inner sequence is sent as is, the wrapper is not.
#
# A pane writes raw bytes (one write per case, markers in writes of their
# own) and a `-C` client records `%output`. For every case the decoded
# stream must hold no `ESC P tmux;`, the inner sequence complete, and the
# redraw after it with its ESCs intact.
#
# Runs under a private -L namespace and PSMUX_DATA_DIR; cleans up with
# `psmux -L <ns> kill-server` only. PSMUX_EXE picks another binary.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i1026_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'i1026_' + [guid]::NewGuid().ToString('N').Substring(0, 6)
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $work
Remove-Item Env:SHELL -ErrorAction SilentlyContinue
Remove-Item Env:TMUX -ErrorAction SilentlyContinue
Remove-Item Env:TMUX_PANE -ErrorAction SilentlyContinue

# The pane program: writes each case's bytes in a single raw write.
$writer = Join-Path $work 'writer.ps1'
@'
$o = [Console]::OpenStandardOutput()
function W([byte[]]$b) { $o.Write($b, 0, $b.Length); $o.Flush() }
function B([string]$s) { [System.Text.Encoding]::ASCII.GetBytes($s) }
$E = [char]27; $BEL = [char]7
$b64 = 'SGkgQ2hyaXMsCgpHb29kIGNhdGNoLg=='
$osc52 = "$E]52;c;$b64$BEL"
$wrapped = "${E}Ptmux;$E$E]52;c;$b64$BEL$E\"
$wrapped1337 = "${E}Ptmux;$E$E]1337;SetUserVar=foo=YmFy$BEL$E\"
$redraw = "$E[?25l$E[5;3HAFTER-REDRAW$E[?25h"
$cases = @(
    @('copy', ($osc52 + $wrapped + $redraw)),
    @('wrapper-only', ($wrapped + $redraw)),
    @('wrapper-1337', ($wrapped1337 + $redraw))
)
Start-Sleep -Seconds 3
foreach ($c in $cases) {
    W (B "<<$($c[0])>>"); Start-Sleep -Milliseconds 200
    W (B $c[1]); Start-Sleep -Milliseconds 200
    W (B "<<end>>`r`n"); Start-Sleep -Milliseconds 300
}
W (B "<<done>>`r`n")
Start-Sleep -Seconds 30
'@ | Set-Content -Encoding ascii $writer

$shell = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $shell) { $shell = (Get-Command powershell -EA Stop).Source }

function Decode-Output([string]$line) {
    # %output %N <tmux octal escaped bytes>
    $parts = $line.Split(' ', 3)
    if ($parts.Count -lt 3) { return , [byte[]]@() }
    $s = $parts[2]; $bytes = [System.Collections.Generic.List[byte]]::new()
    $i = 0
    while ($i -lt $s.Length) {
        if ($s[$i] -eq '\' -and $i + 3 -lt $s.Length + 0 -and $s.Substring($i + 1, [Math]::Min(3, $s.Length - $i - 1)) -match '^[0-7]{3}$') {
            $bytes.Add([Convert]::ToByte($s.Substring($i + 1, 3), 8)); $i += 4
        } elseif ($s[$i] -eq '\' -and $i + 1 -lt $s.Length -and $s[$i + 1] -eq '\') {
            $bytes.Add(92); $i += 2
        } else {
            foreach ($b in [System.Text.Encoding]::UTF8.GetBytes([string]$s[$i])) { $bytes.Add($b) }; $i++
        }
    }
    return , $bytes.ToArray()
}

$cc = $null
$all = ''
try {
    $conf = Join-Path $work 'empty.conf'; Set-Content -Path $conf -Value ''
    & $PSMUX -L $ns -f $conf new-session -d -s s -x 120 -y 30 "`"$shell`" -NoProfile -NoLogo -File `"$writer`""
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $ns -C attach -t s")
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $cc = [System.Diagnostics.Process]::Start($psi)
    $stream = [System.Collections.Generic.List[byte]]::new()
    $latin = [System.Text.Encoding]::GetEncoding(28591)
    $deadline = (Get-Date).AddSeconds(25)
    $task = $null
    while ((Get-Date) -lt $deadline) {
        if ($null -eq $task) { $task = $cc.StandardOutput.ReadLineAsync() }
        if (-not $task.Wait(1000)) { continue }
        $line = $task.Result; $task = $null
        if ($null -eq $line) { break }
        if ($line.StartsWith('%output ')) { $stream.AddRange((Decode-Output $line)) }
        if ($latin.GetString($stream.ToArray()).Contains('<<done>>')) { break }
    }
    $all = $latin.GetString($stream.ToArray())
    $E = [char]27
} catch {
    Write-Fail "harness error: $_"
}
try {
    if (-not $all.Contains('<<done>>')) { Write-Fail 'pane output did not reach %output (no <<done>> marker)' }
    foreach ($name in 'copy', 'wrapper-only', 'wrapper-1337') {
        $i = $all.IndexOf("<<$name>>"); $j = if ($i -ge 0) { $all.IndexOf('<<end>>', $i) } else { -1 }
        if ($i -lt 0 -or $j -lt 0) { Write-Fail "${name}: markers not seen"; continue }
        $body = $all.Substring($i + $name.Length + 4, $j - $i - $name.Length - 4)
        $shown = $body.Replace("$E", '\e').Replace([string][char]7, '\a')
        if ($body.Contains("${E}Ptmux;")) { Write-Fail "${name}: passthrough wrapper reached %output: $shown" }
        else { Write-Pass "${name}: no passthrough wrapper in %output" }
        $inner = if ($name -eq 'wrapper-1337') { "$E]1337;SetUserVar=foo=YmFy$([char]7)" } else { "$E]52;c;SGkgQ2hyaXMsCgpHb29kIGNhdGNoLg==$([char]7)" }
        $want = if ($name -eq 'copy') { 2 } else { 1 }
        $got = ([regex]::Matches($body, [regex]::Escape($inner))).Count
        if ($got -eq $want) { Write-Pass "${name}: $got complete inner sequence(s)" } else { Write-Fail "${name}: $got complete inner sequence(s), want ${want}: $shown" }
        if ($body.Contains("$inner$E[?25l$E[5;3HAFTER-REDRAW")) { Write-Pass "${name}: redraw follows the inner sequence with its ESCs" }
        else { Write-Fail "${name}: redraw not right after the inner sequence: $shown" }
    }
} finally {
    if ($cc -and -not $cc.HasExited) { try { $cc.Kill() } catch {} }
    & $PSMUX -L $ns kill-server 2>$null | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
