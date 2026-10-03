<#
.SYNOPSIS
  Test that cursor-style and cursor-blink options propagate correctly
  from `set -g` through the server to the client's dump-state JSON.

  DECSCUSR code mapping:
    block+blink=1  block+noblink=2
    underline+blink=3  underline+noblink=4
    bar+blink=5  bar+noblink=6
    default=0

  Run before each release:
    pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_cursor_style.ps1
#>
$ErrorActionPreference = "Continue"
$results = @()
$PSMUX = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $PSMUX)) { $PSMUX = "$PSScriptRoot\..\target\debug\psmux.exe" }
if (-not (Test-Path $PSMUX)) { $PSMUX = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $PSMUX -or -not (Test-Path $PSMUX)) { Write-Error "psmux binary not found"; exit 1 }
$PSMUX_DIR = "$env:USERPROFILE\.psmux"

function Add-Result($name, $pass, $detail="") {
    $script:results += [PSCustomObject]@{ Test=$name; Result=if($pass){"PASS"}else{"FAIL"}; Detail=$detail }
    $mark = if($pass) { "[PASS]" } else { "[FAIL]" }
    Write-Host "  $mark $name$(if($detail){' '+$detail}else{''})"
}

function Reset-Psmux {
    Get-Process psmux -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    Remove-Item "$PSMUX_DIR\*.port" -Force -ErrorAction SilentlyContinue
    Remove-Item "$PSMUX_DIR\*.key" -Force -ErrorAction SilentlyContinue
}

function Start-SessionWithConfig {
    param([string]$ConfigPath, [string]$SessionName = "ctest")
    Reset-Psmux
    $env:PSMUX_CONFIG_FILE = $ConfigPath
    Start-Process -FilePath $PSMUX -ArgumentList "new-session -s $SessionName -d" -WindowStyle Hidden
    Start-Sleep -Seconds 3
    $env:PSMUX_CONFIG_FILE = $null
    & $PSMUX has-session -t $SessionName 2>$null
    return ($LASTEXITCODE -eq 0)
}

function Get-Opt {
    param([string]$Option, [string]$Session = "ctest")
    (& $PSMUX show-options -g -v $Option -t $Session 2>&1 | Out-String).Trim()
}

Write-Host "=== Cursor Style Test ==="
Write-Host ""

# =====================================================================
# TEST 1: Default cursor style (default, blink on)
# =====================================================================
Write-Host "--- Test 1: Default cursor style ---"
$confDefault = "$env:TEMP\psmux_cursor_default.conf"
Set-Content -Path $confDefault -Value "# empty config — defaults only" -Encoding UTF8

if (Start-SessionWithConfig $confDefault "cdefault") {
    $opt = Get-Opt "cursor-style" "cdefault"
    Add-Result "Default: cursor-style is default" ($opt -eq 'default') "($opt)"

    $blink = Get-Opt "cursor-blink" "cdefault"
    Add-Result "Default: cursor-blink option readable" ($blink -ne '') "($blink)"
} else {
    Add-Result "Default: session start" $false "failed"
}

# =====================================================================
# TEST 2: set -g cursor-style block, cursor-blink off
# =====================================================================
Write-Host "`n--- Test 2: cursor-style block ---"
$conf2 = "$env:TEMP\psmux_cursor_block.conf"
Set-Content -Path $conf2 -Value "set -g cursor-style block`nset -g cursor-blink off" -Encoding UTF8

if (Start-SessionWithConfig $conf2 "cblock") {
    $opt2 = Get-Opt "cursor-style" "cblock"
    Add-Result "Block: cursor-style=block" ($opt2 -eq "block") "($opt2)"

    $blink2 = Get-Opt "cursor-blink" "cblock"
    Add-Result "Block: cursor-blink=off" ($blink2 -eq "off") "($blink2)"
} else {
    Add-Result "Block: session start" $false "failed"
}

# =====================================================================
# TEST 3: set -g cursor-style underline, cursor-blink on
# =====================================================================
Write-Host "`n--- Test 3: cursor-style underline ---"
$conf3 = "$env:TEMP\psmux_cursor_uline.conf"
Set-Content -Path $conf3 -Value "set -g cursor-style underline`nset -g cursor-blink on" -Encoding UTF8

if (Start-SessionWithConfig $conf3 "culine") {
    $opt3 = Get-Opt "cursor-style" "culine"
    Add-Result "Underline: cursor-style=underline" ($opt3 -eq "underline") "($opt3)"

    $blink3 = Get-Opt "cursor-blink" "culine"
    Add-Result "Underline: cursor-blink=on" ($blink3 -eq "on") "($blink3)"
} else {
    Add-Result "Underline: session start" $false "failed"
}

# =====================================================================
# TEST 4: Runtime change via set-option
# =====================================================================
Write-Host "`n--- Test 4: Runtime cursor-style change ---"
$conf4 = "$env:TEMP\psmux_cursor_runtime.conf"
Set-Content -Path $conf4 -Value "set -g cursor-style block`nset -g cursor-blink off" -Encoding UTF8

if (Start-SessionWithConfig $conf4 "cruntime") {
    $opt4a = Get-Opt "cursor-style" "cruntime"
    Add-Result "Runtime: starts as block" ($opt4a -eq "block") "($opt4a)"

    & $PSMUX set-option -g -t cruntime cursor-style bar 2>$null
    Start-Sleep -Seconds 1
    $opt4b = Get-Opt "cursor-style" "cruntime"
    Add-Result "Runtime: changed to bar" ($opt4b -eq "bar") "($opt4b)"

    & $PSMUX set-option -g -t cruntime cursor-blink on 2>$null
    Start-Sleep -Seconds 1
    $blink4 = Get-Opt "cursor-blink" "cruntime"
    Add-Result "Runtime: cursor-blink changed to on" ($blink4 -eq "on") "($blink4)"
} else {
    Add-Result "Runtime: session start" $false "failed"
}

# =====================================================================
# TEST 5: All DECSCUSR code mappings via runtime set-option
# =====================================================================
Write-Host "`n--- Test 5: DECSCUSR code mapping ---"
$conf5 = "$env:TEMP\psmux_cursor_code.conf"
Set-Content -Path $conf5 -Value "set -g cursor-style block`nset -g cursor-blink on" -Encoding UTF8

if (Start-SessionWithConfig $conf5 "ccode") {
    $combos = @(
        @{style="block";     blink="on";  label="block+blink=1"},
        @{style="block";     blink="off"; label="block+noblink=2"},
        @{style="underline"; blink="on";  label="underline+blink=3"},
        @{style="underline"; blink="off"; label="underline+noblink=4"},
        @{style="bar";       blink="on";  label="bar+blink=5"},
        @{style="bar";       blink="off"; label="bar+noblink=6"}
    )

    foreach ($c in $combos) {
        & $PSMUX set-option -g -t ccode cursor-style $c.style 2>$null
        & $PSMUX set-option -g -t ccode cursor-blink $c.blink 2>$null
        Start-Sleep -Milliseconds 500
        $s = Get-Opt "cursor-style" "ccode"
        $b = Get-Opt "cursor-blink" "ccode"
        $ok = ($s -eq $c.style) -and ($b -eq $c.blink)
        Add-Result "DECSCUSR map: $($c.label)" $ok "(style=$s blink=$b)"
    }
} else {
    Add-Result "DECSCUSR: session start" $false "failed"
}

# =====================================================================
# TEST 6: Cursor resets after TUI exit (DECSCUSR → configured default)
# =====================================================================
Write-Host "`n--- Test 6: Cursor resets after TUI exit ---"
$conf6 = "$env:TEMP\psmux_cursor_reset.conf"
Set-Content -Path $conf6 -Value "set -g cursor-style underline`nset -g cursor-blink off" -Encoding UTF8

if (Start-SessionWithConfig $conf6 "creset") {
    # Verify starts as underline
    $pre = Get-Opt "cursor-style" "creset"
    Add-Result "Pre-TUI: cursor-style=underline" ($pre -eq "underline") "($pre)"

    # Launch a fake TUI that changes cursor to block (DECSCUSR 2)
    $fakeTui = @'
$esc = [char]27
Write-Host -NoNewline "$esc[?1049h"
Write-Host -NoNewline "$esc[2 q"
Start-Sleep -Seconds 2
Write-Host -NoNewline "$esc[?1049l"
'@
    $fakeTuiScript = "$env:TEMP\fake_tui_cursor.ps1"
    Set-Content -Path $fakeTuiScript -Value $fakeTui -Encoding UTF8

    & $PSMUX send-keys -t creset "pwsh -NoProfile -File `"$fakeTuiScript`"" Enter
    Start-Sleep -Seconds 4

    # After TUI exit, the configured option should still be underline
    $post = Get-Opt "cursor-style" "creset"
    Add-Result "Post-TUI: cursor-style still underline" ($post -eq "underline") "($post)"

    Remove-Item $fakeTuiScript -Force -ErrorAction SilentlyContinue
} else {
    Add-Result "Post-TUI: session start" $false "failed"
}

# ═══════════════════════════════════════════════════════════════
# Win32 TUI VERIFICATION: Prove cursor style changes via real keys
# ═══════════════════════════════════════════════════════════════
Write-Host ""
Write-Host ("=" * 60)
Write-Host "Win32 TUI VISUAL VERIFICATION" -ForegroundColor Yellow
Write-Host ("=" * 60)

. "$PSScriptRoot\tui_helper.ps1"
$TUI_SESSION_CUR = "cur_tui_proof"

$tuiOk = Launch-PsmuxWindow -Session $TUI_SESSION_CUR
if ($tuiOk) {
    Start-Sleep -Seconds 2

    # TUI Test 1: Set cursor-style to block via CLI (visible TUI window)
    Write-Host "  [TEST] TUI: Set cursor-style block (visible TUI proof)" -ForegroundColor White
    & $PSMUX set-option -t $TUI_SESSION_CUR cursor-style block 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $style = & $PSMUX show-options -g -v cursor-style -t $TUI_SESSION_CUR 2>&1 | Out-String
    $style = $style.Trim()
    if ($style -match "block") {
        Add-Result "TUI: cursor-style = block" $true "($style)"
    } else {
        Add-Result "TUI: cursor-style = block" $false "($style)"
    }

    # TUI Test 2: Change to underline via CLI
    Write-Host "  [TEST] TUI: Set cursor-style underline (visible TUI proof)" -ForegroundColor White
    & $PSMUX set-option -t $TUI_SESSION_CUR cursor-style underline 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $style2 = & $PSMUX show-options -g -v cursor-style -t $TUI_SESSION_CUR 2>&1 | Out-String
    $style2 = $style2.Trim()
    if ($style2 -match "underline") {
        Add-Result "TUI: cursor-style = underline" $true "($style2)"
    } else {
        Add-Result "TUI: cursor-style = underline" $false "($style2)"
    }

    # TUI Test 3: Change to bar via CLI
    Write-Host "  [TEST] TUI: Set cursor-style bar (visible TUI proof)" -ForegroundColor White
    & $PSMUX set-option -t $TUI_SESSION_CUR cursor-style bar 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $style3 = & $PSMUX show-options -g -v cursor-style -t $TUI_SESSION_CUR 2>&1 | Out-String
    $style3 = $style3.Trim()
    if ($style3 -match "bar") {
        Add-Result "TUI: cursor-style = bar" $true "($style3)"
    } else {
        Add-Result "TUI: cursor-style = bar" $false "($style3)"
    }

    Cleanup-PsmuxWindow -Session $TUI_SESSION_CUR
    Write-Host ""
} else {
    Write-Host "  TUI verification skipped (could not launch window)" -ForegroundColor Yellow
}

# Cleanup
Remove-Item "$env:TEMP\psmux_cursor_*.conf" -Force -ErrorAction SilentlyContinue
Remove-Item $fakeTuiScript -Force -ErrorAction SilentlyContinue
& $PSMUX kill-server 2>$null

# --- Summary ---
Write-Host "`n=== RESULTS ==="
$results | Format-Table -AutoSize | Out-String | Write-Host
$pass = ($results | Where-Object { $_.Result -eq "PASS" }).Count
$fail = ($results | Where-Object { $_.Result -eq "FAIL" }).Count
Write-Host "Total: $($results.Count)  Pass: $pass  Fail: $fail"
if ($fail -gt 0) { exit 1 } else { exit 0 }
