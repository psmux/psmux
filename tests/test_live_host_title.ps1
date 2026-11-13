#Requires -Version 7.0
<#
.SYNOPSIS
Verify title-only pane output reaches the attached host without a state query.
.DESCRIPTION
Uses a private data directory, unique -L namespace and a headless ConPTY host.
Does not attach to or stop user sessions. Logs and captures remain in OutputDir.
.PARAMETER PsmuxExe
Binary to test.
.PARAMETER OutputDir
Directory for isolated test data and captured output.
.PARAMETER Mode
Osc emits OSC 0; Console uses the Windows console title API.
.PARAMETER ServerMode
Select the per-session or shared server implementation.
.EXAMPLE
./tests/test_live_host_title.ps1 -PsmuxExe ./target/debug/psmux.exe -OutputDir .reviews/temp/title-test
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $PsmuxExe,
    [Parameter(Mandatory)][string] $OutputDir,
    [ValidateSet('Osc', 'Console')][string] $Mode = 'Osc',
    [ValidateSet('per-session', 'shared')][string] $ServerMode = 'per-session'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PsmuxExe = (Resolve-Path -LiteralPath $PsmuxExe).Path
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
[void][IO.Directory]::CreateDirectory($OutputDir)
$namespace = 'title-' + [Guid]::NewGuid().ToString('N')
$oldData = $env:PSMUX_DATA_DIR
$oldConfig = $env:PSMUX_CONFIG_FILE
$oldServerMode = $env:PSMUX_SERVER_MODE
$utf8 = [Text.UTF8Encoding]::new($false)
function Invoke-TestCommand {
    & $PsmuxExe -L $namespace @args
    if ($LASTEXITCODE -ne 0) { throw "psmux command failed: $args" }
}
try {
    $env:PSMUX_SERVER_MODE = $ServerMode
    $env:PSMUX_DATA_DIR = Join-Path $OutputDir $namespace
    $env:PSMUX_CONFIG_FILE = Join-Path $OutputDir 'test.conf'
    [IO.File]::WriteAllText($env:PSMUX_CONFIG_FILE, @'
set -g allow-set-title on
set -g set-titles on
set -g set-titles-string "#{pane_title}"
set -g automatic-rename off
set -g status off
'@, $utf8)
    $spinner = Join-Path $OutputDir 'spinner.ps1'
    [IO.File]::WriteAllText($spinner, @'
param([ValidateSet('Osc', 'Console')][string] $Mode)
Start-Sleep -Seconds 3
for ($i = 0; $i -lt 20; $i++) {
    if ($Mode -eq "Console") {
        [Console]::Title = "TITLE_FRAME_$i"
    } else {
        [Console]::Write("`e]0;TITLE_FRAME_$i`a")
    }
    [Console]::Out.Flush()
    Start-Sleep -Milliseconds 200
}
Start-Sleep -Seconds 30
'@, $utf8)
    $hostExe = Join-Path $OutputDir 'title-host.exe'
    $csc = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    & $csc /nologo "/out:$hostExe" (Join-Path $PSScriptRoot 'conpty_timed_host.cs')
    if ($LASTEXITCODE -ne 0) { throw 'ConPTY host compilation failed' }
    $actions = Join-Path $OutputDir 'actions.txt'
    [IO.File]::WriteAllText($actions, "S 12500`n", $utf8)
    Invoke-TestCommand new-session -d -s probe
    Invoke-TestCommand send-keys -t probe -l "pwsh -NoProfile -File `"$spinner`" -Mode $Mode"
    Invoke-TestCommand send-keys -t probe Enter
    $capture = Join-Path $OutputDir 'capture.bin'
    & $hostExe (Join-Path $OutputDir 'timing.txt') $actions $capture -- "`"$PsmuxExe`"" -L $namespace attach-session -t probe
    if ($LASTEXITCODE -ne 0) { throw 'ConPTY host failed' }
    # Query only AFTER capture: display-message itself synchronizes pane.title
    # and would hide a missing synchronization in the unsolicited frame path.
    $internalTitle = Invoke-TestCommand display-message -p -t probe '#{pane_title}'
    $raw = $utf8.GetString([IO.File]::ReadAllBytes($capture))
    $frames = @([regex]::Matches($raw, '\x1b\][02];TITLE_FRAME_(\d+)(?:\x07|\x1b\\)') |
        ForEach-Object { [int]$_.Groups[1].Value } | Select-Object -Unique)
    $result = [pscustomobject]@{ InternalTitle=$internalTitle; HostFrames=$frames; Count=$frames.Count }
    $result | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDir 'result.json') -Encoding utf8
    $result | ConvertTo-Json
    if ($internalTitle -ne 'TITLE_FRAME_19') { throw 'Generator did not reach its final title' }
    if ($frames.Count -lt 15 -or $frames[-1] -ne 19) { throw 'Host title updates stalled' }
} finally {
    & $PsmuxExe -L $namespace kill-server | Out-Null
    $env:PSMUX_DATA_DIR = $oldData
    $env:PSMUX_CONFIG_FILE = $oldConfig
    $env:PSMUX_SERVER_MODE = $oldServerMode
}
