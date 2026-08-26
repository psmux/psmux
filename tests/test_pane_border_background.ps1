$ErrorActionPreference = "Stop"
$psmux = (Get-Command psmux -ErrorAction Stop).Source
$runId = [guid]::NewGuid().ToString("N")
$namespace = "borderbg_$($runId.Substring(0, 12))"
$session = "borderbg"
$artifactRoot = Join-Path $env:TEMP "psmux-border-bg-$runId"
$hostExe = Join-Path $artifactRoot "conpty_style_host.exe"
$outputFile = Join-Path $artifactRoot "conpty_out.bin"
$controlFile = Join-Path $artifactRoot "conpty_ctrl.txt"
$logFile = Join-Path $artifactRoot "conpty_host.log"
$pidFile = Join-Path $artifactRoot "conpty_childpid.txt"
$needle = "48;5;21"
$failed = 0

function Read-BinEscaped([long]$Start = 0) {
    if (-not (Test-Path $outputFile)) { return "" }
    $stream = [System.IO.File]::Open($outputFile, "Open", "Read", "ReadWrite")
    $length = [Math]::Max(0, $stream.Length - $Start)
    [void]$stream.Seek($Start, [System.IO.SeekOrigin]::Begin)
    $buffer = New-Object byte[] $length
    [void]$stream.Read($buffer, 0, $length)
    $stream.Close()
    $builder = [System.Text.StringBuilder]::new()
    foreach ($byte in $buffer) {
        if ($byte -eq 0x1b) { [void]$builder.Append("<ESC>") }
        elseif ($byte -ge 32 -and $byte -lt 127) { [void]$builder.Append([char]$byte) }
        elseif ($byte -eq 0x0a) { [void]$builder.Append("\n") }
        else { [void]$builder.Append(("<{0:X2}>" -f $byte)) }
    }
    return $builder.ToString()
}

function Wait-Until([scriptblock]$Condition, [int]$TimeoutSeconds, [string]$Failure) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $deadline)
    throw $Failure
}

New-Item -ItemType Directory -Path $artifactRoot | Out-Null
$previousOutput = $env:PSMUX_CONPTY_OUT
$previousControl = $env:PSMUX_CONPTY_CTRL
$previousLog = $env:PSMUX_CONPTY_LOG
$previousPid = $env:PSMUX_CONPTY_PID
$hadOutput = Test-Path Env:\PSMUX_CONPTY_OUT
$hadControl = Test-Path Env:\PSMUX_CONPTY_CTRL
$hadLog = Test-Path Env:\PSMUX_CONPTY_LOG
$hadPid = Test-Path Env:\PSMUX_CONPTY_PID
$client = $null

try {
    $compiler = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    $compilerOutput = & $compiler /nologo /optimize /out:$hostExe tests\conpty_host_0xE.cs 2>&1
    if (-not (Test-Path $hostExe)) {
        throw "ConPTY host compilation failed: $($compilerOutput -join [Environment]::NewLine)"
    }

    $env:PSMUX_CONPTY_OUT = $outputFile
    $env:PSMUX_CONPTY_CTRL = $controlFile
    $env:PSMUX_CONPTY_LOG = $logFile
    $env:PSMUX_CONPTY_PID = $pidFile
    & $psmux -L $namespace kill-server 2>$null | Out-Null
    $client = Start-Process `
        -FilePath $hostExe `
        -ArgumentList "`"$psmux`" -L $namespace new-session -s $session" `
        -PassThru `
        -WindowStyle Hidden

    $portFile = Join-Path $env:USERPROFILE ".psmux\${namespace}__${session}.port"
    Wait-Until { Test-Path $portFile } 10 "Session port file was not created"
    Wait-Until { (Test-Path $outputFile) -and (Get-Item $outputFile).Length -gt 0 } 10 "Initial render was not captured"

    & $psmux -L $namespace set-option -g status off 2>&1 | Out-Null
    $splitOffset = (Get-Item $outputFile).Length
    & $psmux -L $namespace split-window -h -t $session 2>&1 | Out-Null
    Wait-Until {
        $paneCount = (& $psmux -L $namespace display-message -t $session -p '#{window_panes}' 2>$null).Trim()
        $paneCount -eq "2" -and (Get-Item $outputFile).Length -gt $splitOffset
    } 10 "Two-pane render was not observed"

    $before = Read-BinEscaped
    if ($before -match $needle) {
        throw "Background sequence was present before setting pane-active-border-style"
    }
    $offset = (Get-Item $outputFile).Length
    & $psmux -L $namespace set-option -g pane-active-border-style "fg=colour201,bg=colour21" 2>&1 | Out-Null
    Wait-Until {
        (Read-BinEscaped -Start $offset) -match $needle
    } 10 "Background sequence was not emitted after setting pane-active-border-style"
    Write-Host "[PASS] pane-active-border-style background reaches the attached client" -ForegroundColor Green
}
catch {
    $failed++
    Write-Host "[FAIL] $_" -ForegroundColor Red
}
finally {
    if ($null -ne $client -and -not $client.HasExited) {
        Set-Content -Path $controlFile -Value "QUIT`n" -NoNewline -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 300
        Stop-Process -Id $client.Id -Force -ErrorAction SilentlyContinue
    }
    & $psmux -L $namespace kill-server 2>$null | Out-Null
    if ($hadOutput) { $env:PSMUX_CONPTY_OUT = $previousOutput }
    else { Remove-Item Env:\PSMUX_CONPTY_OUT -ErrorAction SilentlyContinue }
    if ($hadControl) { $env:PSMUX_CONPTY_CTRL = $previousControl }
    else { Remove-Item Env:\PSMUX_CONPTY_CTRL -ErrorAction SilentlyContinue }
    if ($hadLog) { $env:PSMUX_CONPTY_LOG = $previousLog }
    else { Remove-Item Env:\PSMUX_CONPTY_LOG -ErrorAction SilentlyContinue }
    if ($hadPid) { $env:PSMUX_CONPTY_PID = $previousPid }
    else { Remove-Item Env:\PSMUX_CONPTY_PID -ErrorAction SilentlyContinue }
    Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
}

exit $failed
