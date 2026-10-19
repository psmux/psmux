# capture-pane -J joins wrapped lines, as tmux does.
#
# psmux captured row by row and then only trimmed each line for -J, so a line
# the pane wrapped still came back in pieces, and a space at the wrap point
# was lost. tmux (cmd-capture-pane.c) turns GRID_STRING_TRIM_SPACES and
# GRID_STRING_EMPTY_CELLS off for -J: each row is the cells the program
# wrote, spaces included, and a wrapped row gets no newline. Every
# expectation here was measured on tmux 3.4 and 3.8 (WSL) with the same text
# in a 20 column pane:
#
#   AAAAAAAAAABBBBBBBBB CCCC | short  x | <red>REDREDREDREDREDREDRE<default>DX
#   世界世界世界世界世界X | 123456789012345678世界
#
#   -pJ              AAAAAAAAAABBBBBBBBB CCCC | short  x | REDREDREDREDREDREDREDX | ...
#   -pJ -S 0 -E 0    "AAAAAAAAAABBBBBBBBB " (the row's cells, the space included)
#   -pJ -S 1 -E 1    CCCC
#   -p  -S 0 -E 1    AAAAAAAAAABBBBBBBBB | CCCC
#
# Spaces at the very end of a line are covered by the Rust unit tests
# (tests-rs/test_capture_pane_join.rs): ConPTY decides whether those reach
# the pane as spaces or as an erase, so a live pane cannot pin them.
#
# The CLI, a capture into a buffer and a control client (-C) are all checked.
# Runs only under -L in a private PSMUX_DATA_DIR; no window is opened. The
# user's clipboard is saved and put back.

$ErrorActionPreference = 'Continue'
$Exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_test_capture_pane_join_$PID"
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$script:pass = 0
$script:fail = 0
function Show([string]$s) { ($s -replace "`e", '^[') }
function Check([string]$name, [bool]$ok, [string]$detail) {
    if ($ok) { $script:pass++; Write-Host "PASS $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "FAIL $name : $(Show $detail)" -ForegroundColor Red }
}

$ns = "test_capture_pane_join_${PID}_$(Get-Random)"
$tmp = Join-Path $env:PSMUX_DATA_DIR "cpj_$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
$W = [string][char]0x4e16 + [string][char]0x754c   # two wide glyphs
$content = "AAAAAAAAAABBBBBBBBB CCCC`nshort  x`n`e[31mREDREDREDREDREDREDRE`e[0mDX`n" +
    ($W * 5) + "X`n123456789012345678$W`n"
$file = Join-Path $tmp "content.txt"
[IO.File]::WriteAllText($file, $content, (New-Object Text.UTF8Encoding $false))
$child = "pwsh -NoProfile -NoLogo -Command `"[Console]::OutputEncoding=[Text.Encoding]::UTF8; Clear-Host; " +
    "[Console]::Out.Write([IO.File]::ReadAllText('$file')); Start-Sleep 120`""
$JOINED = 'AAAAAAAAAABBBBBBBBB CCCC'

function Utf8([scriptblock]$b) {
    $old = [Console]::OutputEncoding
    [Console]::OutputEncoding = [Text.Encoding]::UTF8
    try { @(& $b) } finally { [Console]::OutputEncoding = $old }
}
function Cap([string[]]$a) { Utf8 { & $Exe -L $ns capture-pane -t base @a } }

$savedClip = Get-Clipboard -Raw
& $Exe -L $ns new-session -d -s base -x 20 -y 10 $child | Out-Null
try {
    $ready = $false
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        if ((Cap @('-p')) -join "`n" -match '123456789012345678') { $ready = $true; break }
    }
    Check 'the pane shows the test lines' $ready ((Cap @('-p')) -join ' | ')

    $plain = Cap @('-p')
    Check 'without -J the wrapped line is two rows' ($plain[0] -eq 'AAAAAAAAAABBBBBBBBB' -and $plain[1] -eq 'CCCC') ($plain[0..1] -join ' | ')

    $j = Cap @('-pJ')
    Check '-J joins the wrapped line and keeps the space at the wrap' ($j[0] -eq $JOINED) "[$($j[0])]"
    Check '-J leaves a short line alone' ($j[1] -eq 'short  x') "[$($j[1])]"
    Check '-J joins a styled wrapped line' ($j[2] -eq 'REDREDREDREDREDREDREDX') "[$($j[2])]"
    Check '-J joins wide glyphs wrapped at the edge' ($j[3] -eq (($W * 5) + 'X')) "[$($j[3])]"
    Check '-J joins a wide glyph that ends a full row' ($j[4] -eq "123456789012345678$W") "[$($j[4])]"

    $r0 = @(Cap @('-pJ', '-S', '0', '-E', '0'))
    Check '-J -S 0 -E 0 is the first row, its last cell included' ($r0.Count -eq 1 -and $r0[0] -eq 'AAAAAAAAAABBBBBBBBB ') "[$($r0 -join ' | ')]"
    $r1 = @(Cap @('-pJ', '-S', '1', '-E', '1'))
    Check '-J -S 1 -E 1 is the continuation row' ($r1.Count -eq 1 -and $r1[0] -eq 'CCCC') ($r1 -join ' | ')
    $r02 = Cap @('-pJ', '-S', '0', '-E', '2')
    Check '-J -S 0 -E 2 joins inside the range' ($r02.Count -eq 2 -and $r02[0] -eq $JOINED -and $r02[1] -eq 'short  x') ($r02 -join ' | ')
    $p01 = Cap @('-p', '-S', '0', '-E', '1')
    Check 'a range without -J still splits and trims' ($p01.Count -eq 2 -and $p01[0] -eq 'AAAAAAAAAABBBBBBBBB' -and $p01[1] -eq 'CCCC') ($p01 -join ' | ')

    $jn = Cap @('-pJN')
    Check '-JN is the same as -J' (($jn[0..4] -join "`n") -eq ($j[0..4] -join "`n")) ($jn[0..4] -join ' | ')

    $je = Cap @('-pJe')
    Check '-Je joins the red run with the rest of its line' ($je[2] -match "REDREDREDREDREDREDRE`e\[[0-9;]*mDX") "[$($je[2])]"
    Check '-Je joins the plain wrapped line too' ($je[0] -match [regex]::Escape($JOINED)) "[$($je[0])]"

    # Into the paste buffer instead of stdout.
    & $Exe -L $ns capture-pane -t base -J | Out-Null
    Start-Sleep -Milliseconds 300
    $buf = Utf8 { & $Exe -L $ns show-buffer }
    Check 'capture-pane -J into a buffer joins too' ($buf.Count -ge 2 -and $buf[0] -eq $JOINED -and $buf[1] -eq 'short  x') ($buf[0..1] -join ' | ')

    # A control client runs capture-pane through its own dispatcher.
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = "-L $ns -C attach -t base"
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync()
    Start-Sleep -Milliseconds 800
    $p.StandardInput.WriteLine('capture-pane -p -J -t base -S 0 -E 2')
    $p.StandardInput.Flush()
    Start-Sleep -Milliseconds 1500
    $p.StandardInput.Close()
    if (-not $p.WaitForExit(8000)) { $p.Kill() }
    $cc = @($outTask.Result -split "`r?`n" | Where-Object { $_ -and $_ -notmatch '^%' })
    Check 'a control client gets the joined lines' (($cc -join "`n") -eq "$JOINED`nshort  x") ($cc -join ' | ')
} finally {
    & $Exe -L $ns kill-server 2>$null | Out-Null
    if ($null -ne $savedClip) { Set-Clipboard -Value $savedClip }
}

Write-Host ""
Write-Host "Passed: $script:pass  Failed: $script:fail"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }
