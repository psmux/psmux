$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
$OutputEncoding = [System.Text.UTF8Encoding]::new()
# PSMUX_TEST_BIN picks the binary under test and PSMUX_DATA_DIR the data
# directory, as in the other suites; the port and key files are read from it.
$psmux = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
$dataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE ".psmux" }
$runId = [guid]::NewGuid().ToString("N")
$namespace = "borderpreview_$($runId.Substring(0, 12))"
$session = "borderpreview"
$artifactRoot = Join-Path $env:TEMP "psmux-border-preview-$runId"
$failed = 0

function Wait-Until([scriptblock]$Condition, [int]$TimeoutSeconds, [string]$Failure) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $deadline)
    throw $Failure
}

function Get-DefaultSessions {
    $previousPreference = $PSNativeCommandUseErrorActionPreference
    $PSNativeCommandUseErrorActionPreference = $false
    try {
        return @(& $psmux list-sessions -F '#{session_name}' 2>$null) | Sort-Object
    } finally {
        $PSNativeCommandUseErrorActionPreference = $previousPreference
    }
}

function Invoke-RawPsmux([string]$SessionBase, [string]$Command) {
    $port = [int](Get-Content (Join-Path $dataDir "$SessionBase.port"))
    $key = (Get-Content (Join-Path $dataDir "$SessionBase.key") -Raw).Trim()
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect("127.0.0.1", $port)
        $stream = $client.GetStream()
        $utf8 = [System.Text.UTF8Encoding]::new($false)
        $writer = [System.IO.StreamWriter]::new($stream, $utf8, 1024, $true)
        $reader = [System.IO.StreamReader]::new($stream, $utf8, $false, 1024, $true)
        $writer.NewLine = "`n"
        $writer.WriteLine("AUTH $key")
        $writer.WriteLine($Command)
        $writer.Flush()
        $client.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $authResponse = $reader.ReadLine()
        if ($authResponse -ne "OK") {
            throw "Raw psmux authentication failed: $authResponse"
        }
        return $reader.ReadToEnd()
    } finally {
        $client.Dispose()
    }
}

function Render-Preview([string]$WindowId, [string]$Label) {
    $stdoutPath = Join-Path $artifactRoot "$Label.out"
    $stderrPath = Join-Path $artifactRoot "$Label.err"
    $process = Start-Process `
        -FilePath $psmux `
        -ArgumentList @(
            "-L",
            $namespace,
            "_render-preview",
            "${namespace}__${session}",
            $WindowId,
            "40",
            "12"
        ) `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -Wait `
        -PassThru `
        -WindowStyle Hidden
    $utf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $stdout = $utf8.GetString([System.IO.File]::ReadAllBytes($stdoutPath))
    if ($process.ExitCode -ne 0) {
        $stderr = $utf8.GetString([System.IO.File]::ReadAllBytes($stderrPath))
        throw "_render-preview failed with exit $($process.ExitCode): $stderr"
    }
    return $stdout
}

New-Item -ItemType Directory -Path $artifactRoot | Out-Null
$defaultSessionsBefore = @(Get-DefaultSessions)

try {
    & $psmux -L $namespace kill-server 2>$null | Out-Null
    & $psmux -L $namespace new-session -d -s $session 2>&1 | Out-Null
    $portFile = Join-Path $dataDir "${namespace}__${session}.port"
    Wait-Until { Test-Path $portFile } 10 "Session port file was not created"

    & $psmux -L $namespace split-window -h -t $session 2>&1 | Out-Null
    & $psmux -L $namespace set-option -g '@preview-border' red 2>&1 | Out-Null
    & $psmux -L $namespace set-option -g pane-border-style 'fg=#{@preview-border}' 2>&1 | Out-Null
    & $psmux -L $namespace set-option -g pane-active-border-style fg=blue 2>&1 | Out-Null
    & $psmux -L $namespace set-option -g pane-border-indicators both 2>&1 | Out-Null
    & $psmux -L $namespace set-option -g pane-border-lines spaces 2>&1 | Out-Null

    $windowId = (& $psmux -L $namespace list-windows -t $session -F '#{window_id}').TrimStart("@")
    $sessionBase = "${namespace}__${session}"
    $layoutRaw = Invoke-RawPsmux $sessionBase "window-dump $windowId"
    try {
        $layout = ConvertFrom-Json -InputObject $layoutRaw
    } catch {
        throw "raw window-dump returned invalid JSON: $layoutRaw"
    }
    if ($null -ne $layout.layout -or $null -eq $layout.kind) {
        throw "raw window-dump does not return the layout-only wire format: $layoutRaw"
    }

    $stateRaw = Invoke-RawPsmux $sessionBase "window-dump $windowId state"
    try {
        $state = ConvertFrom-Json -InputObject $stateRaw
    } catch {
        throw "window-dump state returned invalid JSON: $stateRaw"
    }
    if ($state.border_lines -cne "spaces") {
        throw "window-dump state returned border_lines '$($state.border_lines)', expected exactly 'spaces'"
    }
    if ($state.border_indicators -ne "both") {
        throw "window-dump state returned indicators '$($state.border_indicators)', expected 'both'"
    }
    if (
        $state.pane_border_style -ne "fg=red" -or
        $state.pane_active_border_style -ne "fg=blue"
    ) {
        throw "window-dump state did not carry expanded border styles: $stateRaw"
    }
    if ($state.floating_pane_focused) {
        throw "window-dump state reported a focused float before one existed"
    }

    $preview = Render-Preview $windowId "tiled"
    if (-not $preview.Contains([string][char]0x2192)) {
        throw "_render-preview did not use the target window's spaces+both settings"
    }
    if (
        -not $preview.Contains("$([char]27)[31m") -or
        -not $preview.Contains("$([char]27)[34m")
    ) {
        throw "_render-preview did not use the target window's expanded border styles"
    }
    $singleLineGlyphs = @(
        0x2500, 0x2502, 0x250C, 0x2510, 0x2514, 0x2518,
        0x251C, 0x2524, 0x252C, 0x2534, 0x253C
    ) | ForEach-Object { [string][char]$_ }
    $ordinaryBorders = @($singleLineGlyphs | Where-Object { $preview.Contains($_) })
    if ($ordinaryBorders.Count -ne 0) {
        throw "_render-preview emitted single-line glyphs in spaces mode: $($ordinaryBorders -join '')"
    }

    & $psmux -L $namespace set-option -g pane-border-lines single 2>&1 | Out-Null
    & $psmux -L $namespace new-pane -t $session -E -X 2 -Y 2 -x 8 -y 5 2>&1 | Out-Null
    Wait-Until {
        $raw = Invoke-RawPsmux $sessionBase "window-dump $windowId state"
        try {
            (ConvertFrom-Json -InputObject $raw).floating_pane_focused -eq $true
        } catch {
            $false
        }
    } 10 "window-dump state did not report the focused floating pane"

    $floatingPreview = Render-Preview $windowId "floating"
    foreach ($arrow in @(
        [string][char]0x2190,
        [string][char]0x2191,
        [string][char]0x2192,
        [string][char]0x2193
    )) {
        if ($floatingPreview.Contains($arrow)) {
            throw "_render-preview kept tiled arrows while a floating pane was focused"
        }
    }
    if (-not $floatingPreview.Contains("$([char]27)[31m")) {
        throw "_render-preview did not keep the inactive target border style with floating focus"
    }

    Write-Host "[PASS] preview carries pane border settings and floating focus" -ForegroundColor Green
}
catch {
    $failed++
    Write-Host "[FAIL] $($_.InvocationInfo.PositionMessage) $_" -ForegroundColor Red
}
finally {
    & $psmux -L $namespace kill-server 2>$null | Out-Null
    $defaultSessionsAfter = @(Get-DefaultSessions)
    if (Compare-Object $defaultSessionsBefore $defaultSessionsAfter) {
        $failed++
        Write-Host "[FAIL] Default namespace changed during isolated test" -ForegroundColor Red
    }
    Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
}

exit $failed
