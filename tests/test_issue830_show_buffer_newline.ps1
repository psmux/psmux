# Issue #830: show-buffer prints the buffer exactly, with no line break added,
# as in tmux.
#
# A command client's show-buffer writes the buffer to stdout through the same
# file_write as save-buffer, with the buffer's own size and nothing appended
# (cmd-save-buffer.c:108 and :115, tag 3.7c). psmux printed the server's reply
# as it came, and the reply ends with a line break of its own, so `ABC` showed
# as `ABC\n` and `show-buffer > f` then `load-buffer f` grew the buffer.
#
# Every expected value below is what tmux 3.7c wrote for the same commands.
# stdout is read as bytes, because a PowerShell pipeline drops the last line
# break and would hide the difference. Only the CLI is used, under a namespace
# of its own.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$NS  = "bshow-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_bshow_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }

# stdout of one command as bytes, and its exit status.
function Raw {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX)
    foreach ($a in @('-L', $NS) + $args) { $psi.ArgumentList.Add([string]$a) }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $proc = [System.Diagnostics.Process]::Start($psi)
    $ms = [System.IO.MemoryStream]::new()
    $proc.StandardOutput.BaseStream.CopyTo($ms)
    $null = $proc.StandardError.ReadToEnd()
    $proc.WaitForExit()
    return @{ bytes = $ms.ToArray(); rc = $proc.ExitCode }
}
function Hex($bytes) { return (@($bytes) | ForEach-Object { '{0:x2}' -f $_ }) -join ' ' }
function FileHex($path) { return Hex ([System.IO.File]::ReadAllBytes($path)) }
function WriteBytes($path, [byte[]]$bytes) { [System.IO.File]::WriteAllBytes($path, $bytes) }
function ShowHex($name) { return Hex (Raw show-buffer -b $name).bytes }

try {
    P new-session -d -s b -x 80 -y 24 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 150
        & $PSMUX -L $NS has-session -t b 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { break }
    }

    Write-Head "1. set-buffer ABC, then show-buffer"
    P set-buffer -b BF1 ABC | Out-Null
    $h = ShowHex BF1
    Check ($h -eq '41 42 43') "show-buffer -b BF1 writes 41 42 43" "show-buffer -b BF1 writes [$h], tmux writes [41 42 43]"

    Write-Head "2. save-buffer, load-buffer, then show-buffer (the reported steps)"
    $f = Join-Path $TMP 'saved.txt'
    P save-buffer -b BF1 $f | Out-Null
    $h = FileHex $f
    Check ($h -eq '41 42 43') "save-buffer writes 41 42 43" "save-buffer writes [$h]"
    P load-buffer -b BF1 $f | Out-Null
    $h = ShowHex BF1
    Check ($h -eq '41 42 43') "show-buffer after the round trip writes 41 42 43" "show-buffer after the round trip writes [$h], tmux writes [41 42 43]"

    Write-Head "3. show-buffer into a file, then load-buffer that file"
    $f = Join-Path $TMP 'shown.txt'
    WriteBytes $f (Raw show-buffer -b BF1).bytes
    P load-buffer -b BF1 $f | Out-Null
    WriteBytes $f (Raw show-buffer -b BF1).bytes
    P load-buffer -b BF1 $f | Out-Null
    $h = ShowHex BF1
    Check ($h -eq '41 42 43') "two round trips through show-buffer leave 41 42 43" "two round trips through show-buffer leave [$h], tmux leaves [41 42 43]"

    Write-Head "4. the buffer's own line breaks are kept"
    $cases = @(
        @{ name = 'M'; data = [byte[]](0x78, 0x0a, 0x79, 0x0a);             want = '78 0a 79 0a';       label = 'x LF y LF' },
        @{ name = 'C'; data = [byte[]](0x70, 0x0d, 0x0a, 0x71, 0x0d, 0x0a); want = '70 0d 0a 71 0d 0a'; label = 'p CRLF q CRLF' },
        @{ name = 'D'; data = [byte[]](0x61, 0x0a, 0x0a);                   want = '61 0a 0a';          label = 'a LF LF' }
    )
    foreach ($c in $cases) {
        $f = Join-Path $TMP "$($c.name).txt"
        WriteBytes $f $c.data
        P load-buffer -b $c.name $f | Out-Null
        $h = ShowHex $c.name
        Check ($h -eq $c.want) "$($c.label): show-buffer writes $($c.want)" "$($c.label): show-buffer writes [$h], tmux writes [$($c.want)]"
    }

    Write-Head "5. show-buffer without -b, and save-buffer to stdout"
    P set-buffer TOP | Out-Null
    $h = Hex (Raw show-buffer).bytes
    Check ($h -eq '54 4f 50') "show-buffer (top buffer) writes 54 4f 50" "show-buffer (top buffer) writes [$h], tmux writes [54 4f 50]"
    P set-buffer -b S ABC | Out-Null
    $h = Hex (Raw save-buffer -b S -).bytes
    Check ($h -eq '41 42 43') "save-buffer -b S - writes 41 42 43" "save-buffer -b S - writes [$h], tmux writes [41 42 43]"

    Write-Head "6. a missing buffer is still an error"
    $r = Raw show-buffer -b nosuch
    Check ($r.rc -eq 1 -and $r.bytes.Length -eq 0) "show-buffer -b nosuch: exit 1, nothing on stdout" "show-buffer -b nosuch: exit $($r.rc), stdout [$(Hex $r.bytes)]"
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
