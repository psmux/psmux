# Real frame clients with controlled environment hints. No physical ConEmu is required.
param(
    [string]$Exe = $env:PSMUX_EXE,
    [string]$ServerExe,
    [string]$ExpectedConEmuFeatures = '256,RGB,title',
    [string]$ExpectedClassicFeatures = '256,RGB,title'
)
$ErrorActionPreference = 'Stop'
if ($env:PSMUX_DISPOSABLE_GUEST -ne '1' -and $env:PSMUX_TEST_SANDBOX -ne '1' -and $env:GITHUB_ACTIONS -ne 'true') {
    throw 'Run in a disposable Windows guest, test sandbox, or GitHub Actions worker.'
}
if ($PSVersionTable.PSVersion -lt [version]'7.4') { throw 'PowerShell 7.4+ is required for child environment overrides.' }
if (-not $Exe) { $Exe = Join-Path $PSScriptRoot '..\target\release\psmux.exe' }
$Exe = (Resolve-Path $Exe).Path
$ServerExe = if ($ServerExe) { (Resolve-Path $ServerExe).Path } else { $Exe }
$root = Join-Path $env:TEMP ('features763-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $root | Out-Null
$hints = @('ConEmuPID', 'ConEmuANSI', 'WT_SESSION', 'WT_PROFILE_ID', 'TERM_PROGRAM', 'TERM_PROGRAM_VERSION', 'TERMINAL_EMULATOR',
    'WEZTERM_PANE', 'ALACRITTY_LOG', 'SSH_CONNECTION', 'SSH_CLIENT', 'SSH_TTY')
$names = $hints + @('PSMUX_DATA_DIR', 'PSMUX_NO_WARM', 'PSMUX_SERVER_MODE', 'PSMUX_CONFIG_FILE',
    'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'TMUX', 'TMUX_PANE', 'PSMUX_FAKE_WIN_BUILD')
$saved = @{}
function Equal([string]$label, [string]$expected, [string]$actual) {
    if ($expected -cne $actual) { throw "$label expected=[$expected] actual=[$actual]" }
    Write-Host "PASS $label [$actual]"
}
function Px {
    $text = @(& $ServerExe -L $script:ns -f $script:config @args 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "psmux exit=$LASTEXITCODE : $text" }
    $text
}
function Wait-ClientCount([int]$count) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        $actual = (Px display-message -t work -p '#{session_attached}') -join ''
        if ($actual -ceq "$count") { return }
        Start-Sleep -Milliseconds 100
    } while ($watch.Elapsed.TotalSeconds -lt 15)
    throw "Expected $count clients; last session_attached=[$actual]"
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
        $script:ns = 'features763' + [guid]::NewGuid().ToString('N').Substring(0, 12)
        $script:config = Join-Path $env:PSMUX_DATA_DIR 'test.conf'
        Set-Content $script:config 'set -g warm off'
        $clients = @()
        $launchers = @()
        $launcherClients = @{}
        $started = $false
        $primary = $null
        try {
            # A server with ConEmu hints must still report each client's own profile.
            $env:ConEmuPID = '12345'
            $env:ConEmuANSI = 'ON'
            $started = $true
            Px new-session -d -s work -x 80 -y 24 -- cmd.exe | Out-Null
            Wait-ClientCount 0
            Equal "$mode no attached client" '[]' ((Px display-message -t work -p '[#{client_termfeatures}]') -join '')
            foreach ($profile in @('unknown', 'conemu', 'wt', 'wezterm', 'alacritty', 'classic')) {
                $environment = @{}
                foreach ($hint in $hints) { $environment[$hint] = $null }
                if ($profile -eq 'conemu') {
                    $environment['ConEmuPID'] = '12345'
                    $environment['ConEmuANSI'] = 'ON'
                }
                if ($profile -eq 'unknown') { $environment['TERM_PROGRAM'] = 'unrecognized-terminal' }
                if ($profile -eq 'wt') { $environment['WT_SESSION'] = 'terminal-session' }
                if ($profile -eq 'wezterm') {
                    $environment['TERM_PROGRAM'] = 'WezTerm'
                    $environment['TERM_PROGRAM_VERSION'] = '20240203-110809-5046fc22'
                    $environment['WEZTERM_PANE'] = '0'
                }
                if ($profile -eq 'alacritty') { $environment['ALACRITTY_LOG'] = 'Alacritty-123.log' }
                if ($profile -eq 'classic') {
                    $helper = Join-Path $PSScriptRoot 'terminal_feature_client.ps1'
                    $infoPath = Join-Path $env:PSMUX_DATA_DIR 'classic-client.json'
                    $arguments = "-NoProfile -File `"$helper`" -Exe `"$Exe`" -Namespace $script:ns -Config `"$script:config`" -InfoPath `"$infoPath`""
                    $launcher = Start-Process -FilePath (Get-Command pwsh).Source -ArgumentList $arguments -Environment $environment -WindowStyle Hidden -PassThru
                    $launchers += $launcher
                    $watch = [Diagnostics.Stopwatch]::StartNew()
                    while (-not (Test-Path $infoPath)) {
                        if ($launcher.HasExited) { throw "Classic launcher exited: $($launcher.ExitCode)" }
                        if ($watch.Elapsed.TotalSeconds -ge 15) { throw 'Classic launcher did not publish its client identity.' }
                        Start-Sleep -Milliseconds 100
                    }
                    $info = Get-Content $infoPath -Raw | ConvertFrom-Json
                    $client = Get-Process -Id $info.Pid -ErrorAction Stop
                    $null = $client.Handle
                    $launcherClients[$launcher.Id] = $client
                    $clients += $client
                    Equal "$mode classic console precondition" 'ConsoleWindowClass' $info.Class
                } else {
                    $arguments = "-L $script:ns -f `"$script:config`" attach -t work"
                    $client = Start-Process -FilePath $Exe -ArgumentList $arguments -Environment $environment -WindowStyle Hidden -PassThru
                    $clients += $client
                }
                Wait-ClientCount $clients.Count
                if ($client.HasExited) { throw "$profile client exited early: $($client.ExitCode)" }
                $expected = switch ($profile) {
                    'conemu' { $ExpectedConEmuFeatures }
                    'classic' { $ExpectedClassicFeatures }
                    default { '256,RGB,hyperlinks,title' }
                }
                $row = (Px display-message -t work -c "/dev/pts/$($client.Id)" -p '#{client_pid}|#{client_termfeatures}') -join ''
                Equal "$mode $profile client report" "$($client.Id)|$expected" $row
            }
            # Later attaches must not overwrite the first client's report.
            Wait-ClientCount 6
            $first = (Px display-message -t work -c "/dev/pts/$($clients[0].Id)" -p '#{client_pid}|#{client_termfeatures}') -join ''
            Equal "$mode first client's report survives later attaches" "$($clients[0].Id)|256,RGB,hyperlinks,title" $first
            foreach ($client in $clients) {
                if ($mode -eq 'per-session') {
                    Px detach-client -t "/dev/pts/$($client.Id)" | Out-Null
                    if (-not $client.WaitForExit(15000)) { throw 'Client did not exit after directed detach.' }
                    Equal "$mode directed detach exit" '0' "$($client.ExitCode)"
                } else {
                    # The shared command subset has no detach-client; closing the real client must unregister it.
                    $client.Kill()
                    Equal "$mode owned client exits" 'True' "$($client.WaitForExit(15000))"
                }
            }
            Wait-ClientCount 0
            Equal "$mode after detach" '[]' ((Px display-message -t work -p '[#{client_termfeatures}]') -join '')
        } catch {
            $primary = $_
            throw
        } finally {
            $cleanupErrors = @()
            foreach ($client in $clients) {
                try {
                    if (-not $client.HasExited) {
                        $client.Kill()
                        if (-not $client.WaitForExit(15000)) { throw 'Owned client did not exit after termination.' }
                    }
                }
                catch { $cleanupErrors += $_.Exception.Message }
            }
            foreach ($launcher in $launchers) {
                try {
                    if (-not $launcher.WaitForExit(15000)) {
                        $launcher.Kill()
                        if (-not $launcher.WaitForExit(15000)) { throw 'Classic launcher did not exit after termination.' }
                        throw 'Classic launcher did not exit with its client.'
                    }
                    if ($launcherClients.ContainsKey($launcher.Id)) {
                        $expectedExit = $launcherClients[$launcher.Id].ExitCode
                        if ($launcher.ExitCode -ne $expectedExit) {
                            throw "Classic launcher exit=$($launcher.ExitCode), client exit=$expectedExit"
                        }
                    }
                } catch { $cleanupErrors += $_.Exception.Message }
            }
            if ($started) {
                try { Px kill-server | Out-Null } catch { $cleanupErrors += $_.Exception.Message }
            }
            if ($cleanupErrors) {
                if ($primary) { Write-Warning "Cleanup also failed: $cleanupErrors" }
                else { throw "Cleanup failed: $cleanupErrors" }
            }
        }
    }
} finally {
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name])
    }
    # Retain configuration and registry files for diagnostics until the disposable guest is reset.
    Write-Host "Feature detection diagnostics: $root"
}
