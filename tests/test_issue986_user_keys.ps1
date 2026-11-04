# Issue #986: user-keys, tmux's user-defined key sequences (options-table.c:
# 734-743, tty-keys.c:533-540, key-string.c at tag 3.8). Element N is a byte
# sequence the outer terminal sends; that sequence arriving is the key UserN,
# which binds like any other key (`bind User0 resize-pane -L 3`, tmux.1). A
# UserN with no binding, and `send-keys User0`, send nothing to the pane
# (input-keys.c:676). psmux took the name, kept the value as text, and knew no
# User keys.
#
# Each expectation is what tmux 3.8 (`-f /dev/null`, WSL) printed or did for
# the same commands:
#   [1] the option and the key names: show escapes the ESC (`user-keys[0]
#       "\033[5;30012~"`), in the show -s listing too, a value from the
#       command line keeps its backslash (`\\e[96~`), list-keys names
#       User0, M-User1 and User1000, User1001 is an unknown key, and
#       send-keys User0 sends nothing
#   [2] the VT input route (PSMUX_VT_INPUT=1, the route WezTerm, SSH and pipes
#       take): a client under a pseudoconsole is written the sequence, as a
#       terminal's sendInput writes it, and the root and prefix bindings run;
#       ESC and a sequence is M-UserN (tty_keys_next); an unbound user key
#       sends nothing to the pane
#   [3] the console route: the console host parses VT input before psmux reads
#       it (`ESC[5;30012~` arrives as PageUp with modifiers), so a sequence
#       is no user key there; documented, and checked here
#   [4] user-keys set while the client is attached reaches it
#
# The clients run under tests\conpty_vt_keys.cs; the pane records what reaches
# it with tests\console_record_dump.cs. Private USERPROFILE, PSMUX_DATA_DIR and
# config, a -L namespace per server; cleanup is `-L <ns> kill-server` plus the
# host PIDs this suite started. PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:OpenedPids = @()
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_uk_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
$script:NS = ""
function P { (& $PSMUX -L $script:NS @args 2>&1 | ForEach-Object { "$_" }) -join "`n" }

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$VTHOST = Join-Path $TempHome "conpty_vt_keys.exe"
$RECDUMP = Join-Path $TempHome "console_record_dump.exe"
if ($csc) {
    & $csc /nologo /optimize "/out:$VTHOST" (Join-Path $PSScriptRoot "conpty_vt_keys.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$RECDUMP" (Join-Path $PSScriptRoot "console_record_dump.cs") 2>&1 | Out-Null
}

$POWERSHELL = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $POWERSHELL) { $POWERSHELL = (Get-Command powershell).Source }
$BINDINGS = @(
    'set -s user-keys[0] "\e[5;30012~"',
    'set -s user-keys[1] "\e[99~"',
    'set -s user-keys[3] "\eOz"',
    'bind -n User0 set -g @hit0 yes',
    'bind User3 set -g @hit3 yes',
    'bind -n M-User1 set -g @hitm yes',
    'bind -n User1000 display-message z',
    "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'"
)
function Write-Conf($lines) {
    $path = Join-Path $TempHome ("c_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".conf")
    (($lines -join "`n") + "`n") | Set-Content $path -Encoding ASCII
    return $path
}
# The bytes of a string as conpty_vt_keys steps (comma separated hex).
function Hex([string]$s) { ([Text.Encoding]::UTF8.GetBytes($s) | ForEach-Object { '{0:x2}' -f $_ }) -join ',' }
$E = [char]27
$SEQ0 = "$E[5;30012~"; $SEQ1 = "$E[99~"; $SEQ3 = "${E}Oz"

# The characters a recorder in a pane saw, in order (console_record_dump with
# VT input on: one record per character the pane was sent).
function Recorded($file) {
    $chars = @(Get-Content $file -EA SilentlyContinue | Select-String 'DOWN' | ForEach-Object {
        if ($_.Line -match 'ch=0x([0-9A-F]{4})') { [int]("0x" + $Matches[1]) }
    })
    return ($chars | ForEach-Object { if ($_ -eq 27) { 'ESC' } elseif ($_ -lt 32) { '^' + [char]($_ + 64) } else { [string][char]$_ } }) -join ' '
}

# Run a client under the pseudoconsole host. It comes up in 7 s, then the
# host writes $steps (conpty_vt_keys syntax) and holds the console open. The
# pane runs the recorder. $vt picks the input route.
$clientVars = @("PSMUX_SESSION", "PSMUX_SESSION_NAME", "PSMUX_PANE", "PSMUX_TARGET_SESSION",
                "TMUX", "TMUX_PANE", "PSMUX_VT_INPUT", "TERM_PROGRAM", "WEZTERM_PANE", "TERMINAL_EMULATOR", "ALACRITTY_LOG")
function Start-Client($conf, $steps, [bool]$vt, $record) {
    $saved = @{}
    foreach ($k in $clientVars) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); Remove-Item "Env:\$k" -EA SilentlyContinue }
    if ($vt) { Set-Item "Env:\PSMUX_VT_INPUT" "1" }
    $paneCmd = "`"$RECDUMP`" `"$record`" 40 vt"
    $p = Start-Process -FilePath $VTHOST -WindowStyle Hidden -PassThru `
        -ArgumentList @("w7000,$steps,h4000", "`"$PSMUX`"", "-L", $script:NS, "-f", "`"$conf`"", "new-session", "-s", "s", "`"$paneCmd`"")
    $script:OpenedPids += $p.Id
    foreach ($k in $clientVars) { if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -EA SilentlyContinue } }
    return $p
}
function New-Ns { $script:NS = "uk_" + [guid]::NewGuid().ToString('N').Substring(0, 6) }
function Kill-Ns { if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 500 } }

Write-Host "`n=== user-keys ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] The option and the key names" -ForegroundColor Yellow
    New-Ns
    $conf = Write-Conf $BINDINGS
    $env:PSMUX_CONFIG_FILE = $conf
    # The pane runs the recorder, for the send-keys checks at the end.
    $rec = Join-Path $TempHome "rec_send.txt"
    & $PSMUX -L $script:NS new-session -d -s s -x 100 -y 20 "`"$RECDUMP`" `"$rec`" 30 vt" 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $script:NS has-session -t s 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 800
    # tmux quotes a value only when it holds one of ` #';${}%` (or a space or
    # a double quote), so the first element is quoted for its `;` and the
    # others are not.
    Check "show -s escapes the ESC as tmux does" (P show -s user-keys) "user-keys[0] `"\033[5;30012~`"`nuser-keys[1] \033[99~`nuser-keys[3] \033Oz"
    Check "show -sv prints the bytes" (P show -sv 'user-keys[3]') "${E}Oz"
    # The show -s listing without a name carries the same element lines.
    $listed = ((P show -s) -split "`n" | ForEach-Object { $_.TrimEnd("`r") } | Where-Object { $_ -like 'user-keys*' }) -join "`n"
    Check "the show -s listing has the element lines" $listed "user-keys[0] `"\033[5;30012~`"`nuser-keys[1] \033[99~`nuser-keys[3] \033Oz"
    # An argument from the shell is taken as it is, escapes and all: tmux 3.8
    # printed `user-keys[6] \\e[96~` for `set -s 'user-keys[6]' '\e[96~'`.
    P set -s 'user-keys[6]' '\e[96~' | Out-Null
    Check "a command line value keeps its backslash" (P show -s 'user-keys[6]') "user-keys[6] \\e[96~"
    P set -su 'user-keys[6]' | Out-Null
    $root = ((P list-keys -T root) -split "`n" | Where-Object { $_ -match 'User' }) -join "`n"
    Check "list-keys -T root names User0" ([bool]($root -match '-T root +User0 +set(-option)? -g @hit0 yes')) "True"
    Check "list-keys -T root names M-User1" ([bool]($root -match '-T root +M-User1 ')) "True"
    Check "list-keys -T root names User1000 in full (tmux 3.8 prints User100)" ([bool]($root -match '-T root +User1000 ')) "True"
    Check "list-keys -T prefix names User3" ([bool]((P list-keys -T prefix) -match '-T prefix +User3 ')) "True"
    $out = P bind -n User1001 display-message x
    Check "bind User1001 is refused" ([bool]($LASTEXITCODE -ne 0 -and $out -match 'User1001')) "True"
    Check "and binds nothing" ([bool]((P list-keys -T root) -match 'User1001')) "False"
    # send-keys User0 sends nothing to the pane; the x after it arrives.
    P send-keys -t s User0 | Out-Null
    P send-keys -t s M-User1 | Out-Null
    P send-keys -t s x | Out-Null
    Start-Sleep -Milliseconds 1200
    Check "send-keys User0 and M-User1 send nothing, x arrives" (Recorded $rec) "x"
    Check "send-keys User0 and M-User1 run no binding" ("$(P show -gqv '@hit0')$(P show -gqv '@hitm')") ""
    Kill-Ns

    if (-not (Test-Path $VTHOST) -or -not (Test-Path $RECDUMP)) {
        Write-Fail "could not build tests\conpty_vt_keys.cs or tests\console_record_dump.cs (csc.exe unavailable)"
    } else {
        Write-Host "`n[2] The VT input route" -ForegroundColor Yellow
        New-Ns
        $rec = Join-Path $TempHome "rec_vt.txt"
        $steps = "$(Hex $SEQ0),w700,02,$(Hex $SEQ3),w700,$(Hex $SEQ1),w300,1b,$(Hex $SEQ1),w300,79"
        $p = Start-Client $conf $steps $true $rec
        Start-Sleep -Milliseconds 10000
        Check "the sequence of user-keys[0] runs bind -n User0" (P show -gqv '@hit0') "yes"
        Check "the prefix and the sequence of user-keys[3] run bind User3" (P show -gqv '@hit3') "yes"
        Check "ESC and the sequence of user-keys[1] run bind -n M-User1" (P show -gqv '@hitm') "yes"
        Check "the unbound User1 and the bound M-User1 send nothing to the pane, y arrives" (Recorded $rec) "y"
        $p | Wait-Process -Timeout 15 -EA SilentlyContinue
        Kill-Ns

        Write-Host "`n[3] The console route (the console host parses the sequence first)" -ForegroundColor Yellow
        New-Ns
        $rec = Join-Path $TempHome "rec_con.txt"
        $p = Start-Client $conf "$(Hex $SEQ0)" $false $rec
        Start-Sleep -Milliseconds 9000
        Check "the sequence runs no User0 binding on the console route" (P show -gqv '@hit0') ""
        $p | Wait-Process -Timeout 15 -EA SilentlyContinue
        Kill-Ns

        Write-Host "`n[4] user-keys set while the client is attached" -ForegroundColor Yellow
        New-Ns
        $rec = Join-Path $TempHome "rec_live.txt"
        $conf2 = Write-Conf @('bind -n User5 set -g @hit5 yes', "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'")
        $p = Start-Client $conf2 "w2500,$(Hex $SEQ1)" $true $rec
        Start-Sleep -Milliseconds 7800
        # From the command line the value is taken as it is, so the ESC is a
        # real one, as PowerShell writes it.
        P set -s 'user-keys[5]' "$SEQ1" | Out-Null
        Start-Sleep -Milliseconds 3500
        Check "the sequence set after attach runs bind -n User5" (P show -gqv '@hit5') "yes"
        $p | Wait-Process -Timeout 15 -EA SilentlyContinue
        Kill-Ns
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Kill-Ns
    foreach ($id in $script:OpenedPids) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
