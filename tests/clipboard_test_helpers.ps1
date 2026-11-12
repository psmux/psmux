# Clipboard helpers for suites that seed or read the Windows clipboard.
#
# pwsh's Set-Clipboard and Get-Clipboard (7.x, Microsoft.PowerShell.Commands.
# Internal.Clipboard) try OpenClipboard five times back to back with no pause
# and report nothing when it fails: Set-Clipboard becomes a silent no-op and
# Get-Clipboard returns an empty string. Any other window that opens the
# clipboard after a change (a clipboard history listener, a VNC or RDP
# clipboard bridge, a phone link app) makes a one shot set-then-read fail, and
# full sweeps then reported psmux defects that were the test's own seed or read
# not landing (2026-11-11 and 11-12: test_issue198_cv_paste_leak "Could not set
# clipboard to marker (got: )", test_issue298_set_buffer_clipboard "got
# NAMED298", the previous step's value).
#
# Dot-source: . (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")

# Sets the clipboard and reads it back until it holds $Value, for up to
# $TimeoutMs. $true when it does; $false means the desktop kept the clipboard
# from this process, which says nothing about psmux (callers SKIP).
function Set-ClipboardVerified([string]$Value, [int]$TimeoutMs = 5000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try { Set-Clipboard -Value $Value -EA Stop } catch {}
        Start-Sleep -Milliseconds 50
        $now = $(try { Get-Clipboard -Raw -EA Stop } catch { $null })
        if ($null -ne $now -and $now.TrimEnd("`r", "`n") -eq $Value) { return $true }
    }
    return $false
}

# Reads the clipboard text, waiting out a momentary holder: an empty read is
# usually pwsh failing to open a busy clipboard, so it is retried for up to
# $TimeoutMs before the empty answer is believed.
function Get-ClipboardSettled([int]$TimeoutMs = 2000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do {
        $now = $(try { Get-Clipboard -Raw -EA Stop } catch { $null })
        if ($now) { return $now.TrimEnd("`r", "`n") }
        Start-Sleep -Milliseconds 50
    } while ($sw.ElapsedMilliseconds -lt $TimeoutMs)
    return ''
}
