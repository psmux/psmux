# Issue #616: `bind` silently ignores non-ASCII key names.
#
# The reporter runs a Cyrillic layout and wrote
#
#     bind s display-message "latin works"    # OK
#     bind <cyrillic small letter yeru> display-message "cyrillic test"
#
# in the config. The latin line registered, the Cyrillic one vanished: no
# warning at boot, nothing in `list-keys`, and pressing the key did nothing.
# Same from the command line and from the command prompt, and the same for
# modifier forms.
#
# tmux accepts these. key-string.c `key_string_lookup_string` takes the ASCII
# branch only when `string[1] == '\0' && string[0] <= 127`; everything else
# falls to a UTF-8 decode (`utf8_open` / `utf8_append`) which returns the
# codepoint OR'd with the modifiers, so `M-<ef>` and `C-<yeru>` work too, and
# `key_string_lookup_key` prints the raw UTF-8 bytes back, so `list-keys`
# shows the character as typed.
#
# Layers here: config-file boot (the reporter's route), CLI bind-key against a
# live server, modifier forms, unbind-key, an unknown-key error message, and a
# Win32 TUI section where real keystrokes are injected into an attached
# client's console input buffer. Cyrillic cannot be typed by virtual key on a
# US layout, so the injector's {U:XXXX} token is used: a KEY_EVENT_RECORD with
# wVirtualKeyCode 0 and UnicodeChar set, which is exactly what a real IME or
# a Cyrillic layout delivers and what crossterm turns into KeyCode::Char.
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$dataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$TMP = Join-Path $env:TEMP "psmux_616"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = "i616"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# Codepoints, not literals: the script stays pure ASCII on disk so no editor,
# shell or console code page can quietly mangle the very characters under test.
$YERU = [char]0x044B   # CYRILLIC SMALL LETTER YERU     the reporter's key
$EF   = [char]0x0444   # CYRILLIC SMALL LETTER EF       the reporter's M- key
$EACU = [char]0x00E9   # LATIN SMALL LETTER E WITH ACUTE (2 byte UTF-8)
$SZ   = [char]0x00DF   # LATIN SMALL LETTER SHARP S

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "dataDir: $dataDir" -ForegroundColor Cyan
Write-Host "keys:    yeru=U+044B ef=U+0444 eacute=U+00E9 sharps=U+00DF" -ForegroundColor Cyan

function Write-Utf8([string]$path, [string]$text) {
    [IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
}

$emptyConf = Join-Path $TMP "empty.conf"
Write-Utf8 $emptyConf ""

function Kill-Sess([string]$n) {
    & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$dataDir\${SOCK}__$n.*" -Force -EA SilentlyContinue
}
function ListKeys() { (& $PSMUX -L $SOCK list-keys 2>&1 | Out-String) }
function HasBind([string]$table, [string]$keyText, [string]$needle) {
    $lines = (ListKeys) -split "`r?`n"
    foreach ($l in $lines) {
        if ($l -match '\S' -and $l.Contains($keyText) -and $l.Contains($needle)) { return $l.Trim() }
    }
    return $null
}

# ---------------------------------------------------------------------------
# Part A: config file at server boot. This is exactly what the reporter did.
# ---------------------------------------------------------------------------
Write-Host "`n=== Part A: config file boot ===" -ForegroundColor Cyan
$SA = "i616_a"
$confA = Join-Path $TMP "unicode.conf"
Write-Utf8 $confA @"
bind s display-message "latin works"
bind $YERU display-message "cyrillic test"
bind M-$EF display-message "meta cyrillic"
bind C-$YERU display-message "ctrl cyrillic"
bind $EACU display-message "eacute test"
bind $SZ display-message "sharps test"
"@
Write-Info "config bytes: $(([IO.File]::ReadAllBytes($confA)).Length)"

Kill-Sess $SA
Remove-Item "$dataDir\config-warnings.log" -Force -EA SilentlyContinue
$env:PSMUX_CONFIG_FILE = $confA
$bootOut = (& $PSMUX -L $SOCK new-session -d -s $SA -x 120 -y 30 2>&1 | Out-String)
Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
Start-Sleep -Seconds 3

$lk = ListKeys
Write-Info "list-keys lines: $((($lk -split "`r?`n") | Where-Object { $_ -match '\S' }).Count)"

# The control: the latin binding from the SAME file must be there. If this
# fails the config never loaded and every other result below is meaningless.
if (HasBind "prefix" "latin works" "display-message") {
    Write-Pass "control: bind s display-message registered from the config"
} else {
    Write-Fail "control: the latin binding is missing, the config did not load at all"
}

if (HasBind "prefix" $YERU "cyrillic test") {
    Write-Pass "config bind <U+044B> is in list-keys"
} else {
    Write-Fail "config bind <U+044B> is NOT in list-keys (issue #616)"
}
if (HasBind "prefix" $EF "meta cyrillic") {
    Write-Pass "config bind M-<U+0444> is in list-keys"
} else {
    Write-Fail "config bind M-<U+0444> is NOT in list-keys (issue #616)"
}
if (HasBind "prefix" $YERU "ctrl cyrillic") {
    Write-Pass "config bind C-<U+044B> is in list-keys"
} else {
    Write-Fail "config bind C-<U+044B> is NOT in list-keys (issue #616)"
}
if (HasBind "prefix" $EACU "eacute test") {
    Write-Pass "config bind <U+00E9> is in list-keys"
} else {
    Write-Fail "config bind <U+00E9> is NOT in list-keys (issue #616)"
}
if (HasBind "prefix" $SZ "sharps test") {
    Write-Pass "config bind <U+00DF> is in list-keys"
} else {
    Write-Fail "config bind <U+00DF> is NOT in list-keys (issue #616)"
}

# tmux renders the key back as the raw UTF-8 character (key-string.c
# key_string_lookup_key, the KEYC_IS_UNICODE branch), and the modifier forms
# keep their C- / M- prefix. No \u escape, no numeric code.
$row = HasBind "prefix" $YERU "cyrillic test"
if ($row) {
    if ($row -match "(^|\s)$([regex]::Escape([string]$YERU))(\s)") {
        Write-Pass "list-keys renders the key as the bare character: '$row'"
    } else {
        Write-Fail "list-keys renders the key oddly: '$row'"
    }
}
$rowM = HasBind "prefix" $EF "meta cyrillic"
if ($rowM) {
    if ($rowM -match "M-$([regex]::Escape([string]$EF))") {
        Write-Pass "list-keys renders the meta form as M-<char>: '$rowM'"
    } else {
        Write-Fail "list-keys meta form rendered as '$rowM', expected M-<U+0444>"
    }
}

# ---------------------------------------------------------------------------
# Part B: the config path must not be silent about a key it cannot parse.
# tmux says `unknown key: <name>`; psmux funnels config diagnostics through
# config-warnings.log the way #606 did for out of range option values.
# ---------------------------------------------------------------------------
Write-Host "`n=== Part B: an unparseable key is reported, not swallowed ===" -ForegroundColor Cyan
$SB = "i616_b"
$confB = Join-Path $TMP "badkey.conf"
Write-Utf8 $confB "bind NotARealKeyName display-message hi`n"
Kill-Sess $SB
Remove-Item "$dataDir\config-warnings.log" -Force -EA SilentlyContinue
$env:PSMUX_CONFIG_FILE = $confB
$bootB = (& $PSMUX -L $SOCK new-session -d -s $SB -x 120 -y 30 2>&1 | Out-String)
Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
Start-Sleep -Seconds 3
$warnB = ""
if (Test-Path "$dataDir\config-warnings.log") { $warnB = (Get-Content "$dataDir\config-warnings.log" -Raw -Encoding UTF8) }
Write-Info "boot stderr: $((($bootB -split "`r?`n") | Where-Object { $_ -match '\S' }) -join ' | ')"
Write-Info "warnings log: $((($warnB -split "`r?`n") | Where-Object { $_ -match '\S' }) -join ' | ')"
if (($bootB + $warnB) -match 'unknown key: NotARealKeyName') {
    Write-Pass "unparseable key warned like tmux ('unknown key: NotARealKeyName')"
} else {
    Write-Fail "unparseable key was swallowed silently (no 'unknown key' anywhere)"
}
Kill-Sess $SB

# A valid Unicode key must NOT produce a warning.
$SB2 = "i616_b2"
Kill-Sess $SB2
Remove-Item "$dataDir\config-warnings.log" -Force -EA SilentlyContinue
$env:PSMUX_CONFIG_FILE = $confA
$bootB2 = (& $PSMUX -L $SOCK new-session -d -s $SB2 -x 120 -y 30 2>&1 | Out-String)
Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
Start-Sleep -Seconds 3
$warnB2 = ""
if (Test-Path "$dataDir\config-warnings.log") { $warnB2 = (Get-Content "$dataDir\config-warnings.log" -Raw -Encoding UTF8) }
if (($bootB2 + $warnB2) -notmatch 'unknown key') {
    Write-Pass "the Unicode config produces no unknown-key warning"
} else {
    Write-Fail "the Unicode config warned: '$(($bootB2 + $warnB2) -replace "r?n", ' | ')'"
}
Kill-Sess $SB2

# ---------------------------------------------------------------------------
# Part C: CLI bind-key / unbind-key against the live server.
# ---------------------------------------------------------------------------
Write-Host "`n=== Part C: CLI bind-key / unbind-key ===" -ForegroundColor Cyan
$outC = (& $PSMUX -L $SOCK bind-key $YERU display-message "cli cyrillic" 2>&1 | Out-String).Trim()
$rcC = $LASTEXITCODE
Start-Sleep -Milliseconds 500
if ($rcC -eq 0 -and (HasBind "prefix" $YERU "cli cyrillic")) {
    Write-Pass "CLI bind-key <U+044B> registered (rc=$rcC)"
} else {
    Write-Fail "CLI bind-key <U+044B> failed: rc=$rcC out='$outC', not in list-keys"
}

# The modifier form goes down the same server route. It is a DIFFERENT key
# from the bare one, which the unbind check below relies on.
$outCm = (& $PSMUX -L $SOCK bind-key "M-$EF" display-message "cli meta cyrillic" 2>&1 | Out-String).Trim()
$rcCm = $LASTEXITCODE
Start-Sleep -Milliseconds 400
if ($rcCm -eq 0 -and (HasBind "prefix" $EF "cli meta cyrillic")) {
    Write-Pass "CLI bind-key M-<U+0444> registered (rc=$rcCm)"
} else {
    Write-Fail "CLI bind-key M-<U+0444> failed: rc=$rcCm out='$outCm'"
}

# tmux: cmd-bind-key.c line 67, `cmdq_error(item, "unknown key: %s", ...)`.
$outBad = (& $PSMUX -L $SOCK bind-key NotARealKeyName display-message hi 2>&1 | Out-String).Trim()
$rcBad = $LASTEXITCODE
if ($rcBad -ne 0 -and $outBad -match 'unknown key: NotARealKeyName') {
    Write-Pass "CLI rejects an unknown key like tmux: rc=$rcBad '$outBad'"
} else {
    Write-Fail "CLI unknown key -> rc=$rcBad out='$outBad' (expected non-zero and 'unknown key: NotARealKeyName')"
}

# unbind-key must find the same Unicode key it bound.
$outU = (& $PSMUX -L $SOCK unbind-key $YERU 2>&1 | Out-String).Trim()
$rcU = $LASTEXITCODE
Start-Sleep -Milliseconds 500
if ($rcU -eq 0 -and -not (HasBind "prefix" $YERU "cli cyrillic")) {
    Write-Pass "unbind-key <U+044B> removed the binding (rc=$rcU)"
} else {
    Write-Fail "unbind-key <U+044B> did not remove it: rc=$rcU out='$outU'"
}
# The M- form is a DIFFERENT key and must survive the plain unbind, which
# proves unbind resolved the Unicode key rather than clearing the table.
if (HasBind "prefix" $EF "cli meta cyrillic") {
    Write-Pass "unbind of the plain key left M-<U+0444> alone"
} else {
    Write-Fail "unbind of the plain key also removed M-<U+0444>"
}
$outUm = (& $PSMUX -L $SOCK unbind-key "M-$EF" 2>&1 | Out-String).Trim()
Start-Sleep -Milliseconds 500
if (-not (HasBind "prefix" $EF "cli meta cyrillic")) {
    Write-Pass "unbind-key M-<U+0444> removed the modifier form too"
} else {
    Write-Fail "unbind-key M-<U+0444> left it behind: out='$outUm'"
}

Kill-Sess $SA

# ---------------------------------------------------------------------------
# Part D: Win32 TUI. Real keystrokes into an attached client's console input
# buffer. A Cyrillic character has no virtual key on a US layout, so it is
# injected as a KEY_EVENT_RECORD with vk 0 and UnicodeChar set, which is the
# injector's {U:XXXX} token.
# ---------------------------------------------------------------------------
Write-Host "`n=== Part D: attached client, injected Unicode keystrokes ===" -ForegroundColor Cyan
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$keyInj = Join-Path $TMP "keys616.exe"
# Always recompile. A stale injector built before the CharSet.Unicode fix in
# injector.cs sends '?' instead of the Cyrillic character and this whole
# section then measures nothing.
Remove-Item $keyInj -Force -EA SilentlyContinue
& $csc /nologo /optimize /out:$keyInj (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $keyInj)) {
    Write-Fail "could not compile tests/injector.cs, skipping the TUI section"
} else {
    $SD = "i616_d"
    $tuiConf = Join-Path $TMP "tui616.conf"
    Write-Utf8 $tuiConf "set -g mouse off`nbind $YERU new-window`n"
    $launchCmd = Join-Path $TMP "launch616.cmd"
    @"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
set PSMUX_NO_WARM=1
set PSMUX_DATA_DIR=$dataDir
"$PSMUX" -L $SOCK -f "$tuiConf" new-session -s %1 -x 120 -y 30 cmd
"@ | Set-Content -Path $launchCmd -Encoding ASCII

    Kill-Sess $SD
    $null = Start-Process -FilePath $launchCmd -ArgumentList $SD -PassThru
    for ($i = 0; $i -lt 100; $i++) {
        if ((Test-Path "$dataDir\${SOCK}__$SD.port") -or (Test-Path "$dataDir\$SD.port")) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Seconds 5
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$SD\b" } | Select-Object -First 1
    if (-not $cli) {
        Write-Fail "attached client did not start, skipping the TUI section"
    } else {
        $cpid = [int]$cli.ProcessId
        Write-Info "attached client pid=$cpid"
        function WinCount() {
            $v = (& $PSMUX -L $SOCK display-message -p -t $SD "#{session_windows}" 2>&1 | Out-String).Trim()
            if ($v -match '^\d+$') { return [int]$v }
            return -1
        }

        # D1: the config-file binding fires on a real Cyrillic keystroke.
        $before = WinCount
        & $keyInj $cpid "^b{SLEEP:500}{U:044B}" 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        $after = WinCount
        Write-Info "injector log: $((Get-Content "$env:TEMP\psmux_inject.log" -Raw -EA SilentlyContinue) -replace "`r?`n", ' | ')"
        Write-Info "windows $before -> $after"
        if ($after -eq $before + 1) {
            Write-Pass "prefix + U+044B fired the config binding, windows $before -> $after"
        } else {
            Write-Fail "prefix + U+044B did nothing, windows $before -> $after (issue #616)"
        }

        # D2: a binding created at runtime over the CLI fires the same way.
        & $PSMUX -L $SOCK set-option -g "@i616fired" no 2>&1 | Out-Null
        & $PSMUX -L $SOCK bind-key $EF set-option -g "@i616fired" yes 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        & $keyInj $cpid "^b{SLEEP:500}{U:0444}" 2>&1 | Out-Null
        Start-Sleep -Seconds 2
        $fired = (& $PSMUX -L $SOCK show-options -gv "@i616fired" 2>&1 | Out-String).Trim()
        if ($fired -eq "yes") {
            Write-Pass "prefix + U+0444 fired a CLI-created binding (@i616fired=$fired)"
        } else {
            Write-Fail "prefix + U+0444 did not fire, @i616fired='$fired'"
        }

        # D3: `bind` typed at the TUI command prompt with a Unicode key.
        # Opening the prompt, typing and Enter go in separate injector runs so
        # the prompt is painted before the first character arrives. The command
        # is kept short and flagless: every extra injected character is another
        # chance for the console to drop one and turn this into a flake.
        & $keyInj $cpid "^b{SLEEP:800}:" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        & $keyInj $cpid "bind {U:00E9} new-window" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        & $keyInj $cpid "{ENTER}" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $promptRow = HasBind "prefix" $EACU "new-window"
        if ($promptRow) {
            Write-Pass "command prompt bind <U+00E9> ... registered: '$promptRow'"
        } else {
            Write-Fail "command prompt bind <U+00E9> ... did not register"
        }
        $b3 = WinCount
        & $keyInj $cpid "^b{SLEEP:500}{U:00E9}" 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        $a3 = WinCount
        if ($a3 -eq $b3 + 1) {
            Write-Pass "the prompt-created Unicode binding fires, windows $b3 -> $a3"
        } else {
            Write-Fail "the prompt-created Unicode binding did not fire, windows $b3 -> $a3"
        }

        # D4: after unbind the key must fall through to the pane again.
        & $PSMUX -L $SOCK unbind-key $YERU 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        $b4 = WinCount
        & $keyInj $cpid "^b{SLEEP:500}{U:044B}" 2>&1 | Out-Null
        Start-Sleep -Seconds 2
        $a4 = WinCount
        if ($a4 -eq $b4) {
            Write-Pass "after unbind, prefix + U+044B no longer opens a window ($b4 -> $a4)"
        } else {
            Write-Fail "after unbind the key still fired, windows $b4 -> $a4"
        }

        try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
    }
    Kill-Sess $SD
}

Write-Host "`n=== Issue #616 results: $script:Pass passed, $script:Fail failed ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 }
exit 0
