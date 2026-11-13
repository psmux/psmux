#Requires -Version 7.0
<#
.SYNOPSIS
Animates a terminal title without printing ordinary terminal output.
.DESCRIPTION
Run in your terminal with and without psmux. Watch its tab or window title and
avoid changing focus. Repeat in a background tab.
For psmux enable allow-set-title, set-titles and set-titles-string "#{pane_title}".
Osc writes OSC 0 directly; Console uses the Windows console title API, like
Windows CLI applications. Optional output helps identify redraw-dependent lag.
Cleanup restores the console API title; a previous OSC-only title cannot be queried.
Use a disposable terminal if you need to preserve its exact original tab label.
.PARAMETER IntervalMs
Delay between title updates, in milliseconds.
.PARAMETER DurationSeconds
Test duration, in seconds. Ctrl+C stops early and restores the original title.
.PARAMETER Mode
Osc sends an escape sequence; Console sets Console.Title.
.PARAMETER WithOutput
Also print each frame to compare title-only and ordinary-output refresh.
.EXAMPLE
pwsh -NoProfile -File ./tests/animate-terminal-title.ps1 -IntervalMs 150
.EXAMPLE
pwsh -NoProfile -File ./tests/animate-terminal-title.ps1 -Mode Console -WithOutput
#>
[CmdletBinding()]
param(
    [ValidateRange(20, 10000)][int] $IntervalMs = 150,
    [ValidateRange(1, 3600)][int] $DurationSeconds = 60,
    [ValidateSet('Osc', 'Console')][string] $Mode = 'Osc',
    [switch] $WithOutput
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$originalTitle = [Console]::Title
$originalEncoding = [Console]::OutputEncoding
$frames = @(0x280B, 0x2819, 0x2839, 0x2838, 0x283C, 0x2834, 0x2826, 0x2827, 0x2807, 0x280F)
$timer = [Diagnostics.Stopwatch]::StartNew()
$frame = 0

function Set-TestTitle([string] $Title) {
    if ($Mode -eq 'Console') {
        [Console]::Title = $Title
    } else {
        # Strip control characters before constructing an OSC sequence.
        $safeTitle = $Title -replace '[\x00-\x1F\x7F]', ''
        [Console]::Write("`e]0;$safeTitle`a")
        [Console]::Out.Flush()
    }
}

try {
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    while ($timer.Elapsed.TotalSeconds -lt $DurationSeconds) {
        $icon = [char] $frames[$frame % $frames.Count]
        $title = '{0} Title refresh test | frame {1:D4} | {2}' -f $icon, $frame, $Mode
        Set-TestTitle $title
        if ($WithOutput) { [Console]::WriteLine($title) }
        $frame++
        Start-Sleep -Milliseconds $IntervalMs
    }
} finally {
    Set-TestTitle $originalTitle
    [Console]::OutputEncoding = $originalEncoding
}
