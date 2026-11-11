# test_issue720_nu_rehome.ps1 - a nushell pane must be rehomed in nushell's own language
#
# Issue #720: with `set -g default-shell nu`, every pane that psmux moved into a
# start directory by TYPING a cd line (a warm pane claimed by new-window -c or
# split-window -c, or a warm server claimed from another directory) got the
# PowerShell rehome line:
#
#   cd 'C:\code'; try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {}; cls
#   Error: nu::parser::env_var_not_var
#     x Use $env.PWD instead of $PWD.
#
# nu refuses to parse the whole line, so the cd never runs and the pane stays
# where the pool spawned it. `#{pane_current_path}` cannot be used to judge
# this: until the shell's directory moves, psmux deliberately reports the
# REQUESTED directory (the #615 cwd hint), so it read "right" even on the
# broken build. The only honest witness is nu itself, asked with `pwd`.
#
# Sections:
#   1. new-window -c <dir with a space>
#   2. split-window -c <dir with a space>
#   3. new-window -c <dir with an apostrophe>   (nu single quotes cannot hold one)
#   4. new-window -c <it's dir\table\unicode>  (double quoted form: \t \u must
#      stay literal path characters, not escapes)
#   5. new-window -c <non ASCII dir>
#   6. new-window -c C:\  (drive root)
#   7. warm server claimed from a different directory (no -c)
#
# tmux parity: tmux never types into a shell; it chdir()s the forked child
# before exec (spawn.c). psmux hands out shells that are already running (the
# warm pool), so it has to type a cd in the shell's own language. What is
# testable is the outcome: the shell IS in the requested directory and nothing
# stray is on screen.
#
# SKIPS cleanly when nushell (nu.exe) is not installed.
#
# SAFETY: PSMUX_DATA_DIR, USERPROFILE/HOME (for the config file) and nushell's
# config dir all point into a throwaway fixture root, every command runs under
# its own -L namespace, and cleanup is `kill-server` on that namespace only.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source

$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

# ---- locate nushell; without it there is nothing to test --------------------
$NU = (Get-Command nu.exe -EA SilentlyContinue).Source
if (-not $NU) {
    $NU = @(
        (Join-Path $env:LOCALAPPDATA "Programs\nu\bin\nu.exe"),
        "C:\Program Files\nu\bin\nu.exe",
        (Join-Path $env:USERPROFILE ".cargo\bin\nu.exe")
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $NU) {
    Write-Host "  [SKIP] nushell (nu.exe) not installed - #720 needs nu to test" -ForegroundColor Yellow
    exit 0
}
Write-Host "  Using nu: $NU ($((& $NU --version) -join ''))" -ForegroundColor DarkGray
Write-Host "  Using psmux: $PSMUX ($((& $PSMUX -V) -join ' '))" -ForegroundColor DarkGray

# ---- isolated roots + fixture dirs ------------------------------------------
$TAG = [guid]::NewGuid().ToString('N').Substring(0, 8)
$NS = "t720_$TAG"
# NOT under %TEMP%: Storage Sense deletes empty directories there (#600).
$FixtureBase = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "psmux-test-fixtures" } else { $env:TEMP }
$ROOT = Join-Path $FixtureBase "psmux-i720-$TAG"
$FAKEHOME = Join-Path $ROOT "home"
$NUCFG = Join-Path $ROOT "nucfg"
$DATA = Join-Path $ROOT "data"
$BASE = Join-Path $ROOT "base"
$DIRA = Join-Path $ROOT "dirA"
$DIRB = Join-Path $ROOT "dirB"
$SPACE = Join-Path $ROOT "target dir"
$APOS = Join-Path $ROOT "it's here"
$ESCS = Join-Path $ROOT "new's dir\table\unicode"
$UNI = Join-Path $ROOT ([string]::new([char[]](0x00FC, 0x006E, 0x00EF, 0x0020, 0x65E5, 0x672C)))
$DRIVE = [IO.Path]::GetPathRoot($env:SystemRoot)
$all = @($FAKEHOME, (Join-Path $NUCFG "nushell"), $DATA, $BASE, $DIRA, $DIRB, $SPACE, $APOS, $ESCS, $UNI)
New-Item -ItemType Directory -Force -Path $all | Out-Null
# A file in each so nothing that sweeps EMPTY directories takes them.
foreach ($d in $all) { Set-Content -LiteralPath (Join-Path $d "keep.txt") -Value "720" }

# The default config search path, so the warm SERVER claim is exercised too
# (a client with PSMUX_CONFIG_FILE set never claims a warm server).
Set-Content -LiteralPath (Join-Path $FAKEHOME ".psmux.conf") -Value "set -g default-shell nu`n" -Encoding ascii

$saved = @{}
foreach ($v in 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SESSION', 'PSMUX_SESSION_NAME', 'PSMUX_TARGET_SESSION', 'TMUX', 'USERPROFILE', 'HOME', 'XDG_CONFIG_HOME', 'PATH') {
    $saved[$v] = [Environment]::GetEnvironmentVariable($v)
}

$WarmLoadMs = 5000

function P { & $PSMUX -L $NS @args 2>&1 }
function Capture($target) { return ((P capture-pane -p -J -S -3000 -t $target) -join "`n") }

# nu's default prompt ends in "> " after the directory.
function Wait-Prompt($target, [int]$TimeoutMs = 25000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $o = Capture $target
        if ($o -match '(?m)>\s*(\S.*)?$') { return $o }
        Start-Sleep -Milliseconds 200
    }
    return (Capture $target)
}

function Norm($p) { return $p.TrimEnd('\', '/').ToLowerInvariant() }

function Assert-NuRehome($label, $target, $expectedDir) {
    if (-not (Test-Path -LiteralPath $expectedDir)) {
        Write-Fail "$label not judged: fixture $expectedDir was removed by something outside psmux"
        return
    }
    Wait-Prompt $target | Out-Null
    Start-Sleep -Milliseconds 1500   # let the injected line run
    $cap = Capture $target
    $dirty = @()
    if ($cap -match 'nu::parser')          { $dirty += "nu parse error ($([regex]::Match($cap, 'nu::parser::\w+').Value))" }
    if ($cap -match 'SetCurrentDirectory') { $dirty += "PowerShell rehome line typed into nu" }
    if ($cap -match 'unrecognized escape') { $dirty += "unescaped backslash in a nu double quoted string" }
    if ($cap -match 'nu::shell::directory_not_found|directory_not_found') { $dirty += "nu could not find the directory" }
    if ($dirty.Count -eq 0) {
        Write-Pass "$label leaves no nu error in the pane"
    } else {
        Write-Fail "$label left: $($dirty -join ', ')"
        Write-Host (($cap -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 12) -join "`n") -ForegroundColor DarkYellow
    }

    $marker = "I720PWD"
    P send-keys -t $target "print (`"$marker=[`" + (pwd) + `"]`")" Enter | Out-Null
    $answer = "(?m)^$marker=\[(.*)\]\s*$"
    $deadline = (Get-Date).AddSeconds(15)
    $seen = ""
    while ((Get-Date) -lt $deadline) {
        $seen = Capture $target
        if ($seen -match $answer) { break }
        Start-Sleep -Milliseconds 250
    }
    if ($seen -match $answer) {
        $actual = $Matches[1]
        if ((Norm $actual) -eq (Norm $expectedDir)) {
            Write-Pass "$label nu pwd is the requested dir ($actual)"
        } else {
            Write-Fail "$label nu pwd is '$actual', expected '$expectedDir'"
        }
    } else {
        Write-Fail "$label could not read pwd back from the nu pane"
        Write-Host (($seen -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 8) -join "`n") -ForegroundColor DarkYellow
    }
}

try {
    $env:PSMUX_DATA_DIR = $DATA
    $env:USERPROFILE = $FAKEHOME
    $env:HOME = $FAKEHOME
    $env:XDG_CONFIG_HOME = $NUCFG        # nu's config and history stay in the fixture
    $env:PATH = (Split-Path $NU) + ";" + $env:PATH
    Remove-Item Env:\PSMUX_CONFIG_FILE, Env:\PSMUX_SESSION, Env:\PSMUX_SESSION_NAME, Env:\PSMUX_TARGET_SESSION, Env:\TMUX -EA SilentlyContinue

    Write-Host "`n--- base session (cold, in $BASE) ---" -ForegroundColor Cyan
    Push-Location $BASE
    P new-session -d -s s720 -x 250 -y 40 | Out-Null
    Pop-Location
    $b = Wait-Prompt "s720"
    if ($b -match '>') { Write-Pass "nu session created" } else { Write-Fail "nu session did not start: $b" }
    Start-Sleep -Milliseconds $WarmLoadMs

    $cases = @(
        @{ n = "1. new-window -c (space)";               d = $SPACE },
        @{ n = "3. new-window -c (apostrophe)";          d = $APOS },
        @{ n = "4. new-window -c (apostrophe + \t \u)";  d = $ESCS },
        @{ n = "5. new-window -c (non ASCII)";           d = $UNI },
        @{ n = "6. new-window -c (drive root)";          d = $DRIVE }
    )
    $w = 0
    foreach ($c in $cases) {
        Write-Host "`n--- $($c.n): $($c.d) ---" -ForegroundColor Cyan
        $w++
        P new-window -t s720 -c $c.d | Out-Null
        Assert-NuRehome $c.n "s720:$w" $c.d
        if ($w -eq 1) {
            Write-Host "`n--- 2. split-window -c (space): $SPACE ---" -ForegroundColor Cyan
            Start-Sleep -Milliseconds $WarmLoadMs
            P split-window -t s720:1 -c $SPACE | Out-Null
            Assert-NuRehome "2. split-window -c (space)" "s720:1.1" $SPACE
        }
        Start-Sleep -Milliseconds $WarmLoadMs   # let the pool replenish and load
    }

    Write-Host "`n--- 7. warm server claimed from another directory ---" -ForegroundColor Cyan
    Push-Location $DIRA
    P warmup | Out-Null
    Pop-Location
    Start-Sleep -Milliseconds ($WarmLoadMs + 2000)
    Push-Location $DIRB
    P new-session -d -s w720 | Out-Null
    Pop-Location
    Assert-NuRehome "7. warm server claim" "w720" $DIRB
}
finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    Remove-Item -LiteralPath $ROOT -Recurse -Force -EA SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed Failed: $script:TestsFailed"
exit $script:TestsFailed
