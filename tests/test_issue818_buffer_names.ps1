# Issue #818: an automatic paste buffer gets its name once, when it is made,
# and -b finds a buffer by that name, as in tmux.
#
# tmux names an automatic buffer "buffer" and a counter that only goes up,
# skipping a name in use (paste_add, paste.c:178 to :184, tag 3.7c), and looks
# every -b up by name (paste_get_name, paste.c:124 to :134). list-buffers prints
# #{buffer_name} (cmd-list-buffers.c:30), so the plain listing and -F agree.
# set-buffer with empty data adds no buffer (paste.c:165, cmd-set-buffer.c:110).
#
# Every expected value below is what tmux 3.7c printed for the same commands
# (the tables in the issue). Only the CLI is used, under a namespace of its own.
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

$NS  = "b818-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_b818_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }
# Output and exit status of one command.
function PX { $o = (& $PSMUX -L $NS @args 2>&1 | ForEach-Object { "$_" }) -join "`n"; return @{ out = $o.Trim(); rc = $LASTEXITCODE } }
function Lines { return @(P list-buffers | ForEach-Object { "$_" } | Where-Object { $_.Trim() }) }
function Names { return @(P list-buffers -F '#{buffer_name}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) }
function Show($name) { return ((P show-buffer -b $name) -join "`n") }

# A fresh server for each table, so the counter starts at 0 as in the issue.
function Fresh {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    P new-session -d -s b -x 80 -y 24 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 150
        & $PSMUX -L $NS has-session -t b 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { break }
    }
}

try {
    # -- Table 1 --
    Write-Head "1. AUTO1, AUTO2, then -b named NAMED"
    Fresh
    P set-buffer AUTO1 | Out-Null
    P set-buffer AUTO2 | Out-Null
    P set-buffer -b named NAMED | Out-Null

    $lines = Lines
    $want = @('buffer1: 5 bytes: "AUTO2"', 'buffer0: 5 bytes: "AUTO1"', 'named: 5 bytes: "NAMED"')
    $missing = @($want | Where-Object { $lines -notcontains $_ })
    Check ($missing.Count -eq 0 -and $lines.Count -eq 3) "list-buffers: $($lines -join ' / ')" "list-buffers: $($lines -join ' / ')"

    $names = Names
    $plainNames = @($lines | ForEach-Object { ($_ -split ':')[0] })
    Check (($names -join ',') -eq ($plainNames -join ',') -and ($names | Sort-Object) -join ',' -eq 'buffer0,buffer1,named') `
        "list-buffers -F names agree with the listing: $($names -join ',')" `
        "-F gives [$($names -join ',')], the listing [$($plainNames -join ',')]"

    $v = Show buffer0
    Check ($v -ceq 'AUTO1') "show-buffer -b buffer0 is AUTO1" "show-buffer -b buffer0 is [$v]"

    $r = PX show-buffer -b buffer0000
    Check ($r.rc -ne 0 -and $r.out -match 'no buffer buffer0000') "show-buffer -b buffer0000: [$($r.out)] exit $($r.rc)" "show-buffer -b buffer0000: [$($r.out)] exit $($r.rc)"

    $r = PX show-buffer -b 0
    Check ($r.rc -ne 0 -and $r.out -match 'no buffer 0') "show-buffer -b 0: [$($r.out)] exit $($r.rc)" "show-buffer -b 0: [$($r.out)] exit $($r.rc)"

    P delete-buffer -b buffer0 | Out-Null
    $lines = Lines
    Check (($lines -join '|') -notmatch 'AUTO1' -and ($lines -join '|') -match 'AUTO2') "delete-buffer -b buffer0 deletes AUTO1" "after delete-buffer -b buffer0: $($lines -join ' / ')"

    P set-buffer AUTO3 | Out-Null
    $a3 = Show buffer2; $a2 = Show buffer1
    Check ($a3 -ceq 'AUTO3' -and $a2 -ceq 'AUTO2') "AUTO3 is buffer2 and AUTO2 stays buffer1" "buffer2 is [$a3], buffer1 is [$a2]"

    # -- Table 2 --
    Write-Head "2. set-buffer -b with an automatic buffer's name"
    Fresh
    P set-buffer AUTO1 | Out-Null
    P set-buffer AUTO2 | Out-Null
    P set-buffer -b buffer0 REPLACED | Out-Null
    $lines = Lines
    Check ($lines.Count -eq 2 -and $lines -contains 'buffer0: 8 bytes: "REPLACED"' -and $lines -contains 'buffer1: 5 bytes: "AUTO2"') `
        "set-buffer -b buffer0 REPLACED replaces it: $($lines -join ' / ')" "after set-buffer -b buffer0: $($lines -join ' / ')"

    P set-buffer -a -b buffer1 +APP | Out-Null
    $lines = Lines
    $v = Show buffer1
    Check ($lines.Count -eq 2 -and $v -ceq 'AUTO2+APP') "set-buffer -a -b buffer1 appends: [$v]" "buffer1 is [$v], listing $($lines -join ' / ')"

    $f = Join-Path $TMP "save.txt"
    P save-buffer -b buffer1 $f | Out-Null
    $saved = if (Test-Path $f) { [IO.File]::ReadAllText($f) } else { '<no file>' }
    Check ($saved -ceq 'AUTO2+APP') "save-buffer -b buffer1 writes AUTO2+APP" "save-buffer -b buffer1 wrote [$saved]"

    P send-keys -t b "# " | Out-Null
    P paste-buffer -b buffer1 -t b | Out-Null
    Start-Sleep -Milliseconds 800
    $cap = (P capture-pane -t b -p) -join "`n"
    Check ($cap -match '# AUTO2\+APP') "paste-buffer -b buffer1 pastes AUTO2+APP" "the pane reads: $(($cap -split "`n" | Where-Object { $_.Trim() }) -join ' | ')"
    P send-keys -t b C-c | Out-Null

    # -- Table 3 --
    Write-Head "3. a buffer named with a number"
    Fresh
    P set-buffer AUTO1 | Out-Null
    P set-buffer AUTO2 | Out-Null
    P set-buffer -b 1 ONE | Out-Null
    $v = Show 1
    Check ($v -ceq 'ONE') "show-buffer -b 1 is ONE" "show-buffer -b 1 is [$v]"
    P delete-buffer -b 1 | Out-Null
    $lines = Lines
    Check ($lines.Count -eq 2 -and ($lines -join '|') -match 'AUTO1' -and ($lines -join '|') -notmatch 'ONE') `
        "delete-buffer -b 1 deletes the buffer named 1" "after delete-buffer -b 1: $($lines -join ' / ')"

    # -- empty data and the counter --
    Write-Head "4. empty data, and the counter"
    Fresh
    P set-buffer '' | Out-Null
    $lines = Lines
    Check ($lines.Count -eq 0) "set-buffer '' adds no buffer" "after set-buffer '': $($lines -join ' / ')"

    P set-buffer -b buffer0 TAKEN | Out-Null
    P set-buffer A | Out-Null
    $v = Show buffer1
    Check ($v -ceq 'A') "a new buffer skips the name buffer0 that is in use" "buffer1 is [$v], listing $((Lines) -join ' / ')"

    P delete-buffer -b buffer1 | Out-Null
    P set-buffer B | Out-Null
    $n = Names
    Check (($n -contains 'buffer2') -and -not ($n -contains 'buffer1')) "the counter only goes up: B is buffer2" "names after delete and add: $($n -join ',')"
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
