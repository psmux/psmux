# #794: a `-- prog args` pane command runs as an argument vector, straight to
# the program, whatever its program path or its arguments hold.
#
# psmux sent any `--` argv holding & | < > ; ` $ ( ) % to `<shell> -Command`
# as one flattened string: the pane died at once (`&`, `(`, `;`, a backtick,
# any such argument) or the program ran as a child of pwsh (`$`, `%`). tmux
# execvp's an argv of more than one word whatever it holds (spawn.c, job.c),
# and keeps the shell for a single string.
#
# Measured on a68df0aa with copies of tests\argprobe.cs (it records its argv
# and its parent) under directories named plain, a&b, a(b), a$b, a`b, a b,
# a'b, a;b, a%b%, a^b, with fifteen arguments holding the same characters:
#   new-window / split-window / respawn-pane / respawn-window: dead, or pwsh
#   display-popup -E --, new-pane --, a bare respawn of the stored argv: no
#     probe ran; a .cmd under a&b: dead
#   new-session --: already direct
# With the change every pane IS the program (a .cmd script runs under cmd)
# and every argument arrives exactly as given.
#
# GROUND TRUTH: the probe's own record of its argv (CommandLineToArgvW
# rules) and of its parent process, matched to #{pane_pid}. Private
# PSMUX_DATA_DIR and a unique -L namespace; cleanup is `-L <ns> kill-server`,
# then any probe still alive by the pid it wrote.
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
foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Set-Item "env:$v" $null -EA SilentlyContinue }

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_794_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
[void][IO.Directory]::CreateDirectory($TMP)
$env:PSMUX_DATA_DIR = Join-Path $TMP "data"
[void][IO.Directory]::CreateDirectory($env:PSMUX_DATA_DIR)
$OUT = Join-Path $TMP "probes"
[void][IO.Directory]::CreateDirectory($OUT)
$env:ARGPROBE_DIR = $OUT
$NS = "i794-" + [guid]::NewGuid().ToString('N').Substring(0, 6)

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
}
$PROBE = Join-Path $TMP "argprobe.exe"
if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /r:System.Management.dll /out:$PROBE (Join-Path $PSScriptRoot "argprobe.cs") 2>&1 | Out-Null }
if (-not (Test-Path $PROBE)) { Write-Fail "could not build tests\argprobe.cs"; Remove-Item $TMP -Recurse -Force -EA SilentlyContinue; exit 1 }

# The probe under every directory name, plus a .cmd shim beside it and one
# found on PATH by a bare name.
$BASE = Join-Path $TMP "dirs"
$NAMES = @('plain', 'a&b', 'a(b)', 'a$b', 'a`b', 'a b', "a'b", 'a;b', 'a%b%', 'a^b')
foreach ($n in $NAMES) {
    $d = Join-Path $BASE $n
    [void][IO.Directory]::CreateDirectory($d)
    Copy-Item -LiteralPath $PROBE -Destination (Join-Path $d "probe.exe") -Force
    [IO.File]::WriteAllText((Join-Path $d "probe.cmd"), "@`"%~dp0probe.exe`" %*`r`n")
}
$PATHDIR = Join-Path $TMP "pathdir"
[void][IO.Directory]::CreateDirectory($PATHDIR)
[IO.File]::WriteAllText((Join-Path $PATHDIR "i794shim.cmd"), "@`"$BASE\plain\probe.exe`" %*`r`n")
$env:PATH = "$PATHDIR;$env:PATH"
$CONF = Join-Path $TMP "i794.conf"
"set -g remain-on-exit on" | Set-Content $CONF -Encoding ASCII

$ARGV = @('/k', 'x&y', '(p)', '$HOME', '`t', 'sp ace', "q'uote", 'dq"x', 'C:\dir\', 'C:\sp ace\', 'a\\"b', '%PATH%', 'c^d', 'e|f', 'g<h>i')

function Exe($name) { Join-Path (Join-Path $BASE $name) "probe.exe" }
function Records { $h = @{}; foreach ($f in Get-ChildItem $OUT -Filter *.txt -EA SilentlyContinue) { $h[[int]$f.BaseName] = @(Get-Content $f.FullName) }; $h }
function ArgsOf($rec) { @($rec | Where-Object { $_ -like "arg``[*" } | ForEach-Object { $_.Substring($_.IndexOf('=') + 1) }) }
function ParentOf($rec) { (($rec | Where-Object { $_ -like "parent=*" }) -replace '^parent=', '') }
function SameArgs($got, $want) {
    $got = @($got); $want = @($want)
    if ($got.Count -ne $want.Count) { return $false }
    for ($k = 0; $k -lt $want.Count; $k++) { if ($got[$k] -cne $want[$k]) { return $false } }
    return $true
}
function PanePid($target) { [int](((& $PSMUX -L $NS display-message -t $target -p '#{pane_pid}') -join '').Trim()) }
function PaneDead($target) { ((& $PSMUX -L $NS display-message -t $target -p '#{pane_dead}') -join '').Trim() -eq '1' }

# A pane whose process should BE the probe (or cmd running the probe, for a
# script): checks both, and the argv.
function Check-Pane($label, $target, $want, [switch]$Script) {
    Start-Sleep -Milliseconds 300
    if (PaneDead $target) { Write-Fail "${label}: the pane is dead"; return }
    $ppid = PanePid $target
    $recs = Records
    if ($Script) {
        $rec = $null
        foreach ($k in $recs.Keys) { if ((ParentOf $recs[$k]) -match " $ppid$") { $rec = $recs[$k] } }
        $pname = (Get-Process -Id $ppid -EA SilentlyContinue).ProcessName
        if (-not $rec) { Write-Fail "${label}: no probe under the pane's $pname"; return }
        Check ($pname -eq "cmd") "${label}: the pane runs cmd for the script" "${label}: the pane runs $pname"
    } else {
        $rec = $recs[$ppid]
        if (-not $rec) {
            $pname = (Get-Process -Id $ppid -EA SilentlyContinue).ProcessName
            Write-Fail "${label}: the pane's process is $pname, not the program"
            return
        }
        Write-Pass "${label}: the pane's process is the program"
    }
    $got = ArgsOf $rec
    Check (SameArgs $got $want) "${label}: argv exact ($(@($want).Count))" "${label}: argv [$($got -join '|')] wanted [$($want -join '|')]"
}

& $PSMUX -f $CONF -L $NS new-session -d -s t -x 120 -y 30 -- "$env:SystemRoot\System32\cmd.exe" /k | Out-Null
Start-Sleep -Milliseconds 800
& $PSMUX -L $NS has-session -t t 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Fail "the session never came up"; Remove-Item $TMP -Recurse -Force -EA SilentlyContinue; exit 1 }

Write-Head "1. new-window -- <program under a metacharacter directory> /k"
$i = 0
foreach ($n in $NAMES) {
    $i++
    & $PSMUX -L $NS new-window -d -t t -n "p$i" -- (Exe $n) /k | Out-Null
}
Start-Sleep -Seconds 3
$i = 0
foreach ($n in $NAMES) { $i++; Check-Pane "dir $n" "t:p$i" @('/k') }

Write-Head "2. new-window -- <program> with arguments holding the same characters"
$i = 0
foreach ($n in 'plain', 'a&b', 'a$b') {
    $i++
    & $PSMUX -L $NS new-window -d -t t -n "a$i" -- (Exe $n) @ARGV | Out-Null
}
Start-Sleep -Seconds 3
$i = 0
foreach ($n in 'plain', 'a&b', 'a$b') { $i++; Check-Pane "dir $n, 15 arguments" "t:a$i" $ARGV }

Write-Head "3. the other spawn routes"
& $PSMUX -L $NS split-window -d -t t:a2 -- (Exe 'a(b)') @ARGV | Out-Null
Start-Sleep -Seconds 3
Check-Pane "split-window" "t:a2.1" $ARGV

& $PSMUX -L $NS new-window -d -t t -n rp -- "$env:SystemRoot\System32\cmd.exe" /k | Out-Null
& $PSMUX -L $NS new-window -d -t t -n rw -- "$env:SystemRoot\System32\cmd.exe" /k | Out-Null
Start-Sleep -Milliseconds 600
& $PSMUX -L $NS respawn-pane -k -t t:rp -- (Exe 'a;b') @ARGV | Out-Null
& $PSMUX -L $NS respawn-window -k -t t:rw -- (Exe 'a`b') @ARGV | Out-Null
Start-Sleep -Seconds 3
Check-Pane "respawn-pane" "t:rp" $ARGV
Check-Pane "respawn-window" "t:rw" $ARGV

$before = (Records).Keys
& $PSMUX -L $NS respawn-pane -k -t t:rp | Out-Null
Start-Sleep -Seconds 3
Check-Pane "a bare respawn-pane re-runs the stored argv" "t:rp" $ARGV
$start = ((& $PSMUX -L $NS display-message -t t:a1 -p '#{pane_start_command}') -join '')
Check ($start.EndsWith('probe.exe /k x&y (p) "$HOME" `t "sp ace" "q''uote" ''dq"x'' C:\dir\ "C:\sp ace\" ''a\\"b'' "%PATH%" c^d e|f g<h>i')) "#{pane_start_command} quotes like tmux" "#{pane_start_command}: [$start]"

& $PSMUX -f $CONF -L $NS new-session -d -s t2 -x 120 -y 30 -- (Exe 'a%b%') @ARGV | Out-Null
Start-Sleep -Seconds 3
Check-Pane "new-session" "t2:" $ARGV

$recsBefore = Records
& $PSMUX -L $NS display-popup -E -- (Exe 'a&b') @ARGV | Out-Null
Start-Sleep -Seconds 3
$new = (Records).GetEnumerator() | Where-Object { -not $recsBefore.ContainsKey($_.Key) } | Select-Object -First 1
if ($new) {
    Check (SameArgs (ArgsOf $new.Value) $ARGV) "display-popup -E --: argv exact" "display-popup -E --: argv [$((ArgsOf $new.Value) -join '|')]"
    Check ((ParentOf $new.Value) -like "psmux *") "display-popup -E --: run by the server, no shell" "display-popup -E --: parent $(ParentOf $new.Value)"
} else { Write-Fail "display-popup -E --: no probe ran" }
& $PSMUX -L $NS display-popup -C | Out-Null

$recsBefore = Records
& $PSMUX -L $NS new-pane -- (Exe "a'b") @ARGV | Out-Null
Start-Sleep -Seconds 3
$new = (Records).GetEnumerator() | Where-Object { -not $recsBefore.ContainsKey($_.Key) } | Select-Object -First 1
if ($new) {
    Check (SameArgs (ArgsOf $new.Value) $ARGV) "new-pane --: argv exact" "new-pane --: argv [$((ArgsOf $new.Value) -join '|')]"
    Check ((ParentOf $new.Value) -like "psmux *") "new-pane --: run by the server, no shell" "new-pane --: parent $(ParentOf $new.Value)"
} else { Write-Fail "new-pane --: no probe ran" }

Write-Head "4. batch scripts"
& $PSMUX -L $NS new-window -d -t t -n b1 -- (Join-Path (Join-Path $BASE 'a&b') "probe.cmd") @ARGV | Out-Null
& $PSMUX -L $NS new-window -d -t t -n b2 -- i794shim @ARGV | Out-Null
Start-Sleep -Seconds 3
Check-Pane "a&b\probe.cmd" "t:b1" $ARGV -Script
Check-Pane "i794shim (a .cmd found on PATH)" "t:b2" $ARGV -Script

Write-Head "5. a single string keeps the shell"
& $PSMUX -L $NS new-window -d -t t -n s1 "Start-Sleep 30; echo i794" | Out-Null
Start-Sleep -Seconds 2
$sp = PanePid "t:s1"
$sname = (Get-Process -Id $sp -EA SilentlyContinue).ProcessName
Check ($sname -in 'pwsh', 'powershell', 'cmd') "a single string runs in the shell ($sname)" "a single string: pane process [$sname]"

# -- cleanup --
$recs = Records
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
foreach ($id in $recs.Keys) {
    $p = Get-Process -Id $id -EA SilentlyContinue
    if ($p -and $p.Path -like "$BASE*") { Stop-Process -Id $id -Force -EA SilentlyContinue }
}
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
