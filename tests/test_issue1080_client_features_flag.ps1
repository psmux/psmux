# Issue #1080: tmux's `-T features` client flag before the command.
#
# tmux 3.2+ reads `-T features` as what the client's terminal supports
# (tmux.c:476 at tag 3.8) and adds it to the client's features. psmux read the
# flag's value as the command: `psmux -T 256,RGB attach -t =a` ended with
# "unknown command: 256,RGB" at exit 1.
#
# Real clients attach here with ConEmu hints, so the detected features leave
# out `hyperlinks` (see test_issue763_client_feature_detection.ps1) and what
# `-T` adds can be told from what was there already.
param(
    [string]$Exe = $env:PSMUX_EXE
)
$ErrorActionPreference = 'Stop'
if ($env:PSMUX_DISPOSABLE_GUEST -ne '1' -and $env:PSMUX_TEST_SANDBOX -ne '1' -and $env:GITHUB_ACTIONS -ne 'true') {
    throw 'Run in a disposable Windows guest, test sandbox, or GitHub Actions worker.'
}
if ($PSVersionTable.PSVersion -lt [version]'7.4') { throw 'PowerShell 7.4+ is required for child environment overrides.' }
if (-not $Exe) { $Exe = Join-Path $PSScriptRoot '..\target\release\psmux.exe' }
$Exe = (Resolve-Path $Exe).Path
$root = Join-Path $env:TEMP ('features1080-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $root | Out-Null
$hints = @('ConEmuPID', 'ConEmuANSI', 'WT_SESSION', 'WT_PROFILE_ID', 'TERM_PROGRAM', 'TERM_PROGRAM_VERSION', 'TERMINAL_EMULATOR',
    'WEZTERM_PANE', 'ALACRITTY_LOG', 'SSH_CONNECTION', 'SSH_CLIENT', 'SSH_TTY')
$names = $hints + @('PSMUX_DATA_DIR', 'PSMUX_NO_WARM', 'PSMUX_SERVER_MODE', 'PSMUX_CONFIG_FILE',
    'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'TMUX', 'TMUX_PANE', 'PSMUX_FAKE_WIN_BUILD')
$saved = @{}
$script:failed = 0
function Equal([string]$label, [string]$expected, [string]$actual) {
    if ($expected -cne $actual) {
        $script:failed++
        Write-Host "FAIL $label expected=[$expected] actual=[$actual]" -ForegroundColor Red
    } else {
        Write-Host "PASS $label [$actual]"
    }
}
function Px {
    $text = @(& $Exe -L $script:ns -f $script:config @args 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "psmux exit=$LASTEXITCODE : $text" }
    $text
}
function Wait-ClientCount([int]$count) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        $actual = (Px display-message -t work -p '#{session_attached}') -join ''
        if ($actual -ceq "$count") { return $true }
        Start-Sleep -Milliseconds 100
    } while ($watch.Elapsed.TotalSeconds -lt 15)
    $false
}
# Attach one real client with these words before the command, read what the
# server lists for it, and detach it again.
function Client-Features([string]$label, [string[]]$before, [string]$expected) {
    $environment = @{}
    foreach ($hint in $hints) { $environment[$hint] = $null }
    $environment['ConEmuPID'] = '12345'
    $environment['ConEmuANSI'] = 'ON'
    $words = @('-L', $script:ns, '-f', "`"$script:config`"") + $before + @('attach', '-t', 'work')
    $client = Start-Process -FilePath $Exe -ArgumentList ($words -join ' ') -Environment $environment -WindowStyle Hidden -PassThru
    try {
        if (-not (Wait-ClientCount 1)) {
            $exit = if ($client.HasExited) { "exited $($client.ExitCode)" } else { 'still running' }
            Equal $label $expected "no client attached ($exit)"
            return
        }
        $row = (Px display-message -t work -c "/dev/pts/$($client.Id)" -p '#{client_termfeatures}') -join ''
        Equal $label $expected $row
    } finally {
        if (-not $client.HasExited) {
            $client.Kill()
            $null = $client.WaitForExit(15000)
        }
        if (-not (Wait-ClientCount 0)) { throw "$label : the client did not leave" }
    }
}
try {
    foreach ($name in $names) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    foreach ($mode in @('per-session', 'shared')) {
        $env:PSMUX_DATA_DIR = Join-Path $root $mode
        New-Item -ItemType Directory $env:PSMUX_DATA_DIR | Out-Null
        $env:PSMUX_NO_WARM = '1'
        $env:PSMUX_SERVER_MODE = $mode
        $script:ns = 'features1080' + [guid]::NewGuid().ToString('N').Substring(0, 12)
        $script:config = Join-Path $env:PSMUX_DATA_DIR 'test.conf'
        Set-Content $script:config 'set -g warm off'
        $started = $false
        try {
            Px new-session -d -s work -x 80 -y 24 -- cmd.exe | Out-Null
            $started = $true
            if (-not (Wait-ClientCount 0)) { throw 'the session did not start' }

            # The flag is taken by a command that does not attach, too.
            $out = @(& $Exe -L $script:ns -f $script:config -T '256,RGB' list-sessions -F '#{session_name}' 2>&1) -join ''
            Equal "$mode -T before list-sessions: exit" '0' "$LASTEXITCODE"
            Equal "$mode -T before list-sessions: output" 'work' $out
            $out = @(& $Exe -L $script:ns -f $script:config -TRGB has-session -t work 2>&1) -join ''
            Equal "$mode -TRGB before has-session: exit" '0' "$LASTEXITCODE"
            # A -T with nothing after it is refused like the other value flags.
            $out = @(& $Exe -L $script:ns -f $script:config -T 2>&1) -join ''
            Equal "$mode -T with no value: exit" '1' "$LASTEXITCODE"
            Equal "$mode -T with no value: message" 'ERROR: -T expects an argument' $out

            Client-Features "$mode no -T" @() '256,RGB,title,progressbar'
            # The line from the issue: these two are detected already.
            Client-Features "$mode -T 256,RGB" @('-T', '256,RGB') '256,RGB,title,progressbar'
            Client-Features "$mode -T hyperlinks" @('-T', 'hyperlinks') '256,RGB,hyperlinks,title,progressbar'
            Client-Features "$mode -Thyperlinks" @('-Thyperlinks') '256,RGB,hyperlinks,title,progressbar'
            Client-Features "$mode -T rgb:HYPERLINKS" @('-T', 'rgb:HYPERLINKS') '256,RGB,hyperlinks,title,progressbar'
            Client-Features "$mode -T twice" @('-T', '256', '-T', 'hyperlinks') '256,RGB,hyperlinks,title,progressbar'
            Client-Features "$mode a name tmux does not know ends the list" @('-T', 'bogus,hyperlinks') '256,RGB,title,progressbar'
            Client-Features "$mode a feature psmux keeps no bit for" @('-T', 'sync,hyperlinks') '256,RGB,hyperlinks,title,progressbar'
            Client-Features "$mode hyperlinks@ after hyperlinks" @('-T', 'hyperlinks,hyperlinks@') '256,RGB,title,progressbar'

            # terminal-features is read on top of the client's own features.
            Px set -g terminal-features 'xterm*:title' | Out-Null
            Client-Features "$mode terminal-features without progressbar" @() '256,RGB,title'
            Client-Features "$mode -T progressbar" @('-T', 'progressbar') '256,RGB,title,progressbar'
            Px set -as terminal-features '*:progressbar@' | Out-Null
            Client-Features "$mode -T progressbar under progressbar@" @('-T', 'progressbar') '256,RGB,title'
            Px set -su terminal-features | Out-Null
        } finally {
            if ($started) {
                try { Px kill-server | Out-Null } catch { Write-Warning "kill-server: $($_.Exception.Message)" }
            }
        }
    }
} finally {
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name])
    }
    Write-Host "Diagnostics: $root"
}
if ($script:failed -gt 0) {
    Write-Host "$script:failed check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'All checks passed' -ForegroundColor Green
exit 0
