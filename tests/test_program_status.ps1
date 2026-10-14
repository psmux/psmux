# Run only in a disposable Windows VM/account or CI, never the live user session.
param([Parameter(Mandatory)][string]$Binary)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$Binary = (Resolve-Path $Binary).Path
$namespace = 'osc7501_' + [guid]::NewGuid().ToString('N')
$session = 'status'
$work = Join-Path $env:TEMP $namespace
New-Item -ItemType Directory -Path $work | Out-Null
# Prevent inherited pane routing from selecting another session.
foreach ($name in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','PSMUX_PANE_ID','PSMUX_SOCKET','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}
function Default-Sessions {
    $PSNativeCommandUseErrorActionPreference = $false
    (& $Binary list-sessions 2>&1 | Out-String).Trim()
}
function P { & $Binary -L $namespace @args }
# Frames carry tabs and attention; full records come from `list-windows -J`.
function State {
    $state = (P dump-state) | ConvertFrom-Json
    $listing = @((P list-windows -J) | ConvertFrom-Json)
    for ($i = 0; $i -lt $state.windows.Count; $i++) {
        $state.windows[$i] | Add-Member -NotePropertyName ps_panes -NotePropertyValue $listing[$i].ps_panes -Force
    }
    $state
}
function Assert($condition, $message) {
    if (-not $condition) { throw $message }
    Write-Host "PASS: $message"
}
function Wait-State([scriptblock]$condition, [string]$message) {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        $state = State
        if (& $condition $state) { Write-Host "PASS: $message"; return $state }
        Start-Sleep -Milliseconds 30
    } while ([DateTime]::UtcNow -lt $deadline)
    $capture = (P capture-pane -p) -join "`n"
    throw "Timed out: $message`nLast state: $($state | ConvertTo-Json -Depth 20 -Compress)`nPane: $capture"
}
function Open-Client {
    $data = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE '.psmux' }
    $port = [int][IO.File]::ReadAllText((Join-Path $data "${namespace}__${session}.port")).Trim()
    $key = [IO.File]::ReadAllText((Join-Path $data "${namespace}__${session}.key")).Trim()
    $tcp = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $stream = $tcp.GetStream()
    $stream.ReadTimeout = 15000
    $reader = [IO.StreamReader]::new($stream)
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
    $writer.AutoFlush = $true
    $writer.WriteLine("AUTH $key")
    if ($reader.ReadLine() -ne 'OK') { $tcp.Dispose(); throw 'Client authentication failed' }
    $writer.WriteLine('PERSISTENT')
    $writer.WriteLine('dump-state')
    return @{ tcp = $tcp; reader = $reader; writer = $writer }
}
function Wait-Frame($client, [scriptblock]$condition, [string]$message) {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        $line = $client.reader.ReadLine()
        if ($null -eq $line) { throw "Client disconnected: $message" }
        if ($line.StartsWith('{')) {
            $frame = $line | ConvertFrom-Json
            if ($frame.windows -and (& $condition $frame)) { Write-Host "PASS: $message"; return }
        }
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out waiting for pushed frame: $message"
}
$before = Default-Sessions
$clients = @()
$created = $false
try {
    $worker = Join-Path $work 'worker.ps1'
    @'
param([string]$Control)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$previous = ''
[Console]::Write('OSC_WORKER_READY')
while ($true) {
    if (Test-Path $Control) {
        $body = [IO.File]::ReadAllText($Control)
        if ($body -ne $previous) {
            $previous = $body
            if ($body -eq 'exit') { exit 0 }
            if ($body -eq 'output') { [Console]::Write('ordinary output') }
            else { [Console]::Write("$([char]27)]7501;$body$([char]7)") }
            [Console]::Out.Flush()
        }
    }
    Start-Sleep -Milliseconds 10
}
'@ | Set-Content -LiteralPath $worker -Encoding utf8
    $control = Join-Path $work 'report.txt'
    'state=working:app=test' | Set-Content -LiteralPath $control -NoNewline
    $command = 'pwsh -NoLogo -NoProfile -NonInteractive -File "' + $worker + '" -Control "' + $control + '"'
    P -f NUL new-session -d -s $session -x 100 -y 25 $command
    $created = $true
    P set-window-option -g monitor-activity off
    P set-window-option -g monitor-silence 0
    P set-option -g remain-on-exit on
    $state = Wait-State { param($s) $s.windows[0].ps_panes.Count -gt 0 } 'application reports reach the server'
    Assert (-not $state.windows[0].ps_attention) 'working has no attention indicator'
    $pane = $state.windows[0].ps_panes[0].pane
    'state=blocked:app=test' | Set-Content -LiteralPath $control -NoNewline
    $state = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'blocked' } 'stock tabs retain inspectable attention'
    $stockFormat = '#I:#W#{?window_flags,#{window_flags}, }'
    Assert ((P show-options -g -v window-status-format).Trim() -eq $stockFormat) 'inactive default matches tmux'
    Assert ((P show-options -g -v window-status-current-format).Trim() -eq $stockFormat) 'active default matches tmux'
    Assert ($state.windows[0].tab_text -notmatch '\[[?!+]%\d+\]') 'stock tabs omit attention even when blocked'
    $firstWindow = (P display-message -p '#{window_index}').Trim()
    P set-window-option -t "${session}:$firstWindow" window-status-current-format 'LOCAL'
    Assert ((State).windows[0].tab_text -eq 'LOCAL') 'window-local format is installed'
    P set-option -g window-status-format '#I:#W#{?window_flags,#{window_flags}, }#{window_ps_attention}'
    P set-option -g window-status-current-format '#I:#W#{?window_flags,#{window_flags}, }#{window_ps_attention}'
    $state = State
    Assert ($state.windows[0].tab_text -eq 'LOCAL') 'global opt-in preserves a window-local format'
    P set-window-option -t "${session}:$firstWindow" window-status-current-format 'LOCAL#{window_ps_attention}'
    $state = State
    Assert ($state.windows[0].tab_text -match '\[\?%\d+\]') 'local format can opt in to existing attention'
    P set-window-option -u -t "${session}:$firstWindow" window-status-current-format
    P set-option -g -u window-status-format
    P set-option -g -u window-status-current-format
    $state = State
    Assert ((P show-options -g -v window-status-format).Trim() -eq $stockFormat) 'inactive reset matches tmux'
    Assert ((P show-options -g -v window-status-current-format).Trim() -eq $stockFormat) 'active reset matches tmux'
    Assert ($state.windows[0].tab_text -notmatch '\[[?!+]%\d+\]') 'reset withdraws indicators without clearing reports'
    Assert ($state.windows[0].ps_attention.state -eq 'blocked') 'reset preserves inspection'
    P set-option -g window-status-format '#I:#W#{?window_flags,#{window_flags}, }#{window_ps_attention}'
    P set-option -g window-status-current-format '#I:#W#{?window_flags,#{window_flags}, }#{window_ps_attention}'
    $state = State
    Assert ($state.windows[0].tab_text -match '\[\?%\d+\]') 'global format can opt in to existing attention'
    'state=working:app=test' | Set-Content -LiteralPath $control -NoNewline
    $state = Wait-State { param($s) $s.windows[0].ps_panes[0].records[0].state -eq 'working' } 'working report after opt-in'
    Assert ($state.windows[0].tab_text -notmatch '\[[?!+]%\d+\]') 'working remains quiet after opt-in'
    P new-window -t "${session}:" 'pwsh -NoLogo -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 600"'
    $foreground = (P display-message -p '#{window_index}').Trim()
    Assert ($foreground -ne $firstWindow) 'foreground is a separate window'
    $clients = @(Open-Client; Open-Client)
    foreach ($client in $clients) {
        Wait-Frame $client { param($s) $s.windows.Count -gt 0 -and -not $s.windows[0].ps_panes } 'client receives initial state without record text'
    }
    'state=done:app=test:msg=UmVhZHk=' | Set-Content -LiteralPath $control -NoNewline
    $state = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'done' } 'background completion appears with a silent foreground'
    Assert ($state.windows[0].tab_text -match '\[\+%\d+\]') 'completion is a visible text indicator'
    Assert (-not $state.windows[0].activity) 'completion does not set output activity'
    foreach ($client in $clients) {
        Wait-Frame $client { param($s) $s.windows[0].ps_attention.state -eq 'done' } 'background completion is pushed to both clients without polling'
    }
    P copy-mode
    'state=blocked:kind=permission:app=test' | Set-Content -LiteralPath $control -NoNewline
    $state = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'blocked' } 'blocked status updates while foreground is in copy mode'
    Assert ($state.windows[0].tab_text -match '\[\?%\d+\]') 'needs-input indicator is visible'
    P select-window -t "${session}:$firstWindow"
    $state = State
    Assert ($state.windows[0].ps_attention.state -eq 'blocked') 'return does not acknowledge blocked'
    'state=error:app=test' | Set-Content -LiteralPath $control -NoNewline
    $null = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'error' } 'error arriving in the focused pane remains unseen'
    P select-window -t "${session}:$foreground"
    P select-window -t "${session}:$firstWindow"
    $state = State
    Assert (-not $state.windows[0].ps_attention) 'return acknowledges current error'
    Assert ($state.windows[0].ps_panes[0].records[0].state -eq 'error') 'acknowledgement preserves reported state'
    Assert ($state.windows[0].ps_panes[0].records[0].seen) 'acknowledgement is record-local and shared'
    foreach ($client in $clients) {
        Wait-Frame $client { param($s) -not $s.windows[0].ps_attention } 'shared acknowledgement is pushed to both clients'
    }
    'state=working' | Set-Content -LiteralPath $control -NoNewline
    $null = Wait-State { param($s) $s.windows[0].ps_panes[0].records[0].state -eq 'working' } 'next run begins'
    'state=error:app=test' | Set-Content -LiteralPath $control -NoNewline
    $null = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'error' } 'same result after a new run produces fresh attention'
    'state=clear' | Set-Content -LiteralPath $control -NoNewline
    $state = Wait-State { param($s) -not $s.windows[0].ps_attention -and -not $s.windows[0].ps_panes } 'clear withdraws attention and records'
    'state=done' | Set-Content -LiteralPath $control -NoNewline
    $null = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'done' } 'completion before attached process exit'
    'exit' | Set-Content -LiteralPath $control -NoNewline
    $null = Wait-State { param($s) $s.windows[0].ps_attention.state -eq 'done' -and ((P display-message -p -t "%$pane" '#{pane_dead}').Trim() -eq '1') } 'completion survives attached process exit'
    P respawn-pane -E -t "%$pane"
    $null = Wait-State { param($s) -not $s.windows[0].ps_panes } 'respawn withdraws old records and seen state'
} finally {
    foreach ($client in $clients) { $client.tcp.Dispose() }
    if ($created) { P kill-server }
    $after = Default-Sessions
    if ($after -ne $before) { throw "Default session list changed: before=$before after=$after" }
    Remove-Item -LiteralPath $work -Recurse -Force
}
