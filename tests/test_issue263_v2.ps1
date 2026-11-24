# Issue #263 v2: Need to check whether box-drawing chars are reaching the pane,
# and if so, what color attributes their cells carry.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "issue263v2"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:Pass = 0
$script:Fail = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

& $PSMUX new-session -d -s $SESSION -x 120 -y 30 2>$null
Start-Sleep -Seconds 3

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Session creation failed" -F Red; exit 1 }

Write-Host "`n=== Issue #263 v2 ===" -ForegroundColor Cyan

$BAR = [char]0x2502  # │

# Use a heredoc-style file approach: write the test commands to a .ps1 in the
# session's temp dir, then dot-source it. This avoids any send-keys unicode
# encoding issues.
$tmpScript = "$env:TEMP\psmux_box_test.ps1"
$ESC = [char]27
@"
[Console]:: OutputEncoding = [System.Text.Encoding]:: UTF8
`$OutputEncoding = [System.Text.Encoding]:: UTF8
Write-Host "$ESC[90m$BAR SGR 90 brightblack$ESC[0m"
Write-Host "$ESC[37m$BAR SGR 37 white$ESC[0m"
Write-Host "$ESC[1;37m$BAR SGR 1_37 boldwhite$ESC[0m"
Write-Host "$ESC[38;5;240m$BAR SGR 256 240$ESC[0m"
Write-Host "$ESC[38;5;250m$BAR SGR 256 250$ESC[0m"
Write-Host "$ESC[38;2;128;128;128m$BAR SGR tc grey$ESC[0m"
Write-Host "$ESC[38;2;255;0;0m$BAR SGR tc red$ESC[0m"
"@ -replace '\[Console\]:: ','[Console]::' -replace '\$OutputEncoding ','$OutputEncoding ' | Set-Content -LiteralPath $tmpScript -Encoding UTF8

Write-Info "Wrote test script to $tmpScript"
Write-Info "Script content (escaped):"
Get-Content $tmpScript | ForEach-Object {
    $shown = $_ -replace $ESC.ToString(), '\e'
    Write-Host "    $shown"
}

# Make sure the session shell is using UTF-8
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION '[Console]::OutputEncoding=[Text.Encoding]::UTF8' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "& '$tmpScript'" Enter
Start-Sleep -Seconds 3

# Capture WITHOUT -e first (plain text) so we can see if the box chars made it
$capPlain = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
Write-Host "`n--- Plain capture (no -e) ---"
$capPlain -split "`n" | ForEach-Object {
    if ($_ -match "SGR " -or $_ -match $BAR) { Write-Host "    $_" }
}

# Count box chars in plain capture
$boxCount = ([regex]::Matches($capPlain, [regex]::Escape($BAR.ToString()))).Count
Write-Info "Box-drawing char $BAR appeared $boxCount times in capture"

# Capture WITH -e
$capE = & $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Out-String
Write-Host "`n--- Capture WITH -e (ANSI) ---"
$capE -split "`n" | ForEach-Object {
    if ($_ -match "SGR " -or $_ -match $BAR) {
        $shown = $_ -replace $ESC.ToString(), '\e'
        Write-Host "    $shown"
    }
}

# Now find SGR before each | (the actual unicode char)
$rx = [regex]::new("$ESC\[([^m]*)m" + [regex]::Escape($BAR.ToString()))
$matches = $rx.Matches($capE)
Write-Info "Regex matched $($matches.Count) [SGR][BAR] sequences"

# Also try the simpler check: does each line containing "SGR " also have the right SGR set?
$lines = ($capE -split "`n") | Where-Object { $_ -match "SGR " }
Write-Info "Lines with 'SGR ' label: $($lines.Count)"
# On Windows 10 (build 19045) the inbox pseudo console that hosts the pane
# re-encodes these colours before psmux reads them (measured there): 37 is
# its default foreground, so it writes no colour at all; bold brightens
# (1;37 comes out as 1;97); and a 256 colour index becomes the RGB of that
# index (38;5;240 as 38;2;88;88;88). psmux keeps what reaches it, so there
# the box must carry that host's spelling, the same as the text. An
# OpenConsole through PSMUX_CONPTY_DIR passes them unchanged.
$Win10Inbox = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
function Get-HostSgr([string]$sgr) {
    if (-not $Win10Inbox) { return $sgr }
    switch ($sgr) {
        '37' { '0' }
        '1;37' { '1;97' }
        '38;5;196' { '38;2;255;0;0' }
        '38;5;240' { '38;2;88;88;88' }
        '38;5;250' { '38;2;188;188;188' }
        default { $sgr }
    }
}
# Every part of $want is in $sgr. A $want of 0 (that host's default
# foreground) means $sgr only resets.
function Test-SgrHas([string]$sgr, [string]$want) {
    $parts = @($sgr -split ';' | Where-Object { $_ -ne '' })
    if ($want -eq '0') { return @($parts | Where-Object { $_ -ne '0' }).Count -eq 0 }
    foreach ($wp in $want -split ';') { if ($parts -notcontains $wp) { return $false } }
    return $true
}

$expectedColors = @("90","37","1;37","38;5;240","38;5;250","38;2;128;128;128","38;2;255;0;0")

for ($i = 0; $i -lt $lines.Count -and $i -lt $expectedColors.Count; $i++) {
    $line = $lines[$i]
    $want = Get-HostSgr $expectedColors[$i]
    # Find the first SGR sequence on the line that contains the bar OR that contains all wanted parts
    # Capture all SGRs on the line
    $sgrRx = [regex]::new("$ESC\[([^m]*)m")
    $sgrs = $sgrRx.Matches($line) | ForEach-Object { $_.Groups[1].Value }
    $shownLine = $line -replace $ESC.ToString(), '\e'

    # Did the box-drawing char appear?
    if ($line -match [regex]::Escape($BAR.ToString())) {
        Write-Info "Line $($i+1) contains BAR. SGRs found: $($sgrs -join ' | ')"
        # Check if any SGR has the wanted color components
        $found = @($sgrs | Where-Object { Test-SgrHas $_ $want }).Count -gt 0
        # The host's default foreground: no SGR on the line sets a colour.
        if ($want -eq '0') { $found = @($sgrs | Where-Object { -not (Test-SgrHas $_ $want) }).Count -eq 0 }
        if ($found) { Write-Pass "Line $($i+1) [$want]: SGR present" }
        else { Write-Fail "Line $($i+1) [$want]: SGR missing. Line=$shownLine" }
    } else {
        Write-Fail "Line $($i+1) [$want]: BAR ($BAR) NOT in line: $shownLine"
    }
}

# Cleanup
Remove-Item $tmpScript -Force -EA SilentlyContinue
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null

Write-Host "`n=== Result ===" -ForegroundColor Cyan
Write-Host "  Pass: $($script:Pass) / Fail: $($script:Fail)"
exit $script:Fail
