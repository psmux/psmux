# #789 follow up: what a command prompt hands to the command it runs.
#
# 1. `command-prompt -k` answers with the key's NAME, written as tmux's
#    key_string_lookup_key writes it (key-string.c:327, status.c:1439).
#    The copy mode prompt passed on the client's send-key spelling (`left`,
#    `pageup`, `insert`, `f1`, `btab`, and nothing at all for Space), and the
#    prompt a prefix binding opens did not know -k or -1: it waited for Enter.
#    Measured in tmux 3.4 through a nested client, for every key below:
#      Left Right Up Down Home End PPage NPage IC DC F1 F5 F12 Enter Escape
#      BSpace Tab BTab Space C-a M-a C-Left S-Left M-Left C-F5 S-F5 a 1 C-b
#
# 2. The template's quotes are syntax, not part of the value. tmux puts the
#    typed text in raw (cmd_template_replace, cmd.c:841) and its parser removes
#    the quotes. Measured in tmux 3.4 (the same function in 3.7c), on the prefix
#    and the copy mode paths alike:
#      rename-window '%%'      TEST -> TEST   a b -> a b   say "hi" -> say "hi"   a;b -> a;b
#      rename-window "%%"      it's -> it's   say "hi" -> say hi
#      rename-window -- '%%'   TEST -> TEST   a b -> a b
#    The copy mode prompt ran its command through a whitespace split, so it
#    named the window 'TEST' and 'a, quotes included, and `--` was taken as the
#    name on both paths.
#
# GROUND TRUTH: `show -gv @k` and `#{window_name}` after real keystrokes
# (tests\injector.cs, WriteConsoleInput) into an attached client this suite
# starts and tracks by PID. Private PSMUX_DATA_DIR and a unique -L namespace;
# cleanup is `-L <ns> kill-server` plus the client PID, nothing by name.
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

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_kq_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
# Always a private data dir: this suite never reads or writes ~/.psmux.
$env:PSMUX_DATA_DIR = Join-Path $TMP "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$NS   = "kq-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "kq"

function P { & $PSMUX -L $NS @args 2>&1 }

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$INJ = Join-Path $TMP "injector.exe"
if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }
if (-not (Test-Path $INJ)) {
    Write-Fail "could not build tests\injector.cs (csc.exe unavailable)"
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
    exit 1
}

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
$CONF = Join-Path $TMP "kq.conf"
@(
    "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'",
    "set -g mode-keys vi",
    "set -g automatic-rename off",
    "bind K command-prompt -k -p key `"set -g @k '%%'`"",
    "bind -T copy-mode-vi K command-prompt -k -p key `"set -g @k '%%'`"",
    "bind O command-prompt -1 -p one `"set -g @k 'O%%'`"",
    "bind 1 command-prompt -p x `"rename-window '%%'`"",
    "bind 2 command-prompt -p x `"rename-window \`"%%\`"`"",
    "bind 4 command-prompt -p x `"rename-window -- '%%'`"",
    "bind -T copy-mode-vi 1 command-prompt -p x `"rename-window '%%'`"",
    "bind -T copy-mode-vi 2 command-prompt -p x `"rename-window \`"%%\`"`"",
    "bind -T copy-mode-vi 4 command-prompt -p x `"rename-window -- '%%'`""
) | Set-Content -Path $CONF -Encoding ASCII

$client = Start-Process -FilePath $PSMUX -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s",$SESS,"-x","100","-y","30" -PassThru
$script:cpid = $client.Id
$up = $false
for ($i = 0; $i -lt 80; $i++) {
    Start-Sleep -Milliseconds 250
    P has-session -t $SESS | Out-Null
    if ($LASTEXITCODE -eq 0) { $up = $true; break }
}
function Cleanup {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    if (Get-Process -Id $script:cpid -EA SilentlyContinue) { Stop-Process -Id $script:cpid -Force -EA SilentlyContinue }
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}
if (-not $up) { Write-Fail "the attached client never came up"; Cleanup; exit 1 }
Start-Sleep -Seconds 3

function Inj($keys) { & $INJ $script:cpid $keys | Out-Null; Start-Sleep -Milliseconds 400 }
function In-Mode { ((P display-message -t $SESS -p '#{pane_in_mode}') -join '').Trim() }
function Leave-CopyMode { if ((In-Mode) -eq '1') { P send-keys -t $SESS -X cancel | Out-Null; Start-Sleep -Milliseconds 300 } }
function Enter-CopyMode { if ((In-Mode) -ne '1') { P copy-mode -t $SESS | Out-Null; Start-Sleep -Milliseconds 400 } }
function Opt { ((P show -gv '@k') -join '').Trim() }
function Win { ((P display-message -t $SESS -p '#{window_name}') -join '').Trim() }

# -- 1. -k names --

$KEYS = [ordered]@{
    "Left" = "{LEFT}"; "Right" = "{RIGHT}"; "Up" = "{UP}"; "Down" = "{DOWN}";
    "Home" = "{HOME}"; "End" = "{END}"; "PPage" = "{PGUP}"; "NPage" = "{PGDN}";
    "IC" = "{MOD:2D:0000:0000}"; "DC" = "{MOD:2E:0000:0000}";
    "F1" = "{MOD:70:0000:0000}"; "F5" = "{MOD:74:0000:0000}"; "F12" = "{MOD:7B:0000:0000}";
    "Enter" = "{ENTER}"; "Escape" = "{ESC}"; "BSpace" = "{MOD:08:0008:0000}";
    "Tab" = "{MOD:09:0009:0000}"; "BTab" = "{MOD:09:0009:0010}"; "Space" = "{MOD:20:0020:0000}";
    "C-a" = "^a"; "M-a" = "{ALT:a}"; "C-Left" = "{MOD:25:0000:0008}"; "S-Left" = "{MOD:25:0000:0010}";
    "M-Left" = "{MOD:25:0000:0002}"; "C-F5" = "{MOD:74:0000:0008}"; "S-F5" = "{MOD:74:0000:0010}";
    "a" = "a"; "1" = "1"; "C-b" = "^b"
}
foreach ($path in "prefix", "copy") {
    Write-Head "1. command-prompt -k answers with the tmux key name ($path binding)"
    foreach ($want in $KEYS.Keys) {
        # C-b at a copy mode prompt is taken by the client as the prefix key,
        # on master too: that prompt runs in the server. Left for later.
        if ($path -eq "copy" -and $want -eq "C-b") { continue }
        P set -g '@k' UNSET | Out-Null
        if ($path -eq "prefix") { Leave-CopyMode; Inj "^b"; Inj "K" } else { Enter-CopyMode; Inj "K" }
        Inj $KEYS[$want]
        Start-Sleep -Milliseconds 200
        $got = Opt
        Check ($got -ceq $want) "$want -> [$got]" "$want -> [$got], tmux answers [$want]"
        if ($got -eq "UNSET") { Inj "{ESC}" }
    }
}

# -- 2. -1 on the prefix path --

Write-Head "2. command-prompt -1 is answered by one character (prefix binding)"
Leave-CopyMode
P set -g '@k' UNSET | Out-Null
Inj "^b"; Inj "O"; Inj "z"
Start-Sleep -Milliseconds 300
$got = Opt
Check ($got -ceq "Oz") "z answers it: [$got]" "z: [$got], wanted [Oz]"
if ($got -ne "Oz") { Inj "{ESC}" }
P set -g '@k' UNSET | Out-Null
Inj "^b"; Inj "O"; Inj "{ESC}"; Inj "y"
Start-Sleep -Milliseconds 300
$got = Opt
Check ($got -ceq "UNSET") "Escape cancels it" "after Escape the option reads [$got]"

# -- 3. quotes and -- --

$VALUES = [ordered]@{
    "TEST" = "TEST"; "a b" = "a b"; "it's" = "it{U:0027}s";
    'say "hi"' = "say {U:0022}hi{U:0022}"; "a;b" = "a;b"
}
$EXPECT = @{
    "1" = @{ "TEST" = "TEST"; "a b" = "a b"; 'say "hi"' = 'say "hi"'; "a;b" = "a;b" };
    "2" = @{ "TEST" = "TEST"; "a b" = "a b"; "it's" = "it's"; 'say "hi"' = "say hi"; "a;b" = "a;b" };
    "4" = @{ "TEST" = "TEST"; "a b" = "a b" }
}
$TEMPLATE = @{ "1" = "rename-window '%%'"; "2" = 'rename-window "%%"'; "4" = "rename-window -- '%%'" }
foreach ($path in "prefix", "copy") {
    Write-Head "3. the template's quotes are not part of the value ($path binding)"
    foreach ($k in "1", "2", "4") {
        foreach ($label in $VALUES.Keys) {
            if (-not $EXPECT[$k].ContainsKey($label)) { continue }
            P rename-window -t $SESS START | Out-Null
            if ($path -eq "prefix") { Leave-CopyMode; Inj "^b"; Inj $k } else { Enter-CopyMode; Inj $k }
            Inj $VALUES[$label]
            Inj "{ENTER}"
            Start-Sleep -Milliseconds 300
            $got = Win
            $want = $EXPECT[$k][$label]
            Check ($got -ceq $want) "$($TEMPLATE[$k]) <- $label : [$got]" "$($TEMPLATE[$k]) <- $label : [$got], tmux names it [$want]"
        }
    }
}

Cleanup
Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
