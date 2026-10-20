# CLI queries preserve caller encoding; console and pipe control input preserves Unicode.
param([string]$PsmuxExe = $env:PSMUX_EXE)
$ErrorActionPreference = 'Stop'
if (-not $PsmuxExe) { $PsmuxExe = (Get-Command psmux -ErrorAction Stop).Source }
$PsmuxExe = (Resolve-Path -LiteralPath $PsmuxExe).Path
$outerNS = 'unicode-' + [guid]::NewGuid().ToString('N')
$controlNS = $outerNS + '-control'
$tempDir = Join-Path ([IO.Path]::GetTempPath()) $outerNS
$previousRoot = $env:PSMUX_DATA_DIR
$previousNesting = $env:PSMUX_ALLOW_NESTING
$failures = [Collections.Generic.List[string]]::new()
$config = $null
$defaultBefore = $null
$privateRoot = $false
$unicode = 'caf' + [char]0xE9 + [char]0x6F22 + [char]::ConvertFromUtf32(0x1F680)
$block = [string][char]0x2588

# The same UTF-8 process helper runs parent queries and subjects inside ConPTY.
# Only subjects share the caller console; parent commands cannot change its CP.
function Start-Psmux([string[]]$Arguments, [switch]$ShareConsole, [switch]$Terminal, [switch]$PipeInput) {
    $si = [Diagnostics.ProcessStartInfo]::new($PsmuxExe)
    $si.UseShellExecute = $false
    $si.CreateNoWindow = -not $ShareConsole
    $si.RedirectStandardOutput = -not $Terminal
    $si.RedirectStandardError = -not $Terminal
    $si.RedirectStandardInput = $PipeInput
    if (-not $Terminal) {
        $si.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
        $si.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    }
    if ($PipeInput) { $si.StandardInputEncoding = [Text.UTF8Encoding]::new($false) }
    foreach ($argument in $Arguments) { [void]$si.ArgumentList.Add($argument) }
    $p = [Diagnostics.Process]::Start($si)
    [pscustomobject]@{
        Process = $p
        Out = if (-not $Terminal) { $p.StandardOutput.ReadToEndAsync() } else { $null }
        Err = if (-not $Terminal) { $p.StandardError.ReadToEndAsync() } else { $null }
    }
}
function Complete-Psmux($Run) {
    try {
        if (-not $Run.Process.WaitForExit(30000)) {
            $Run.Process.Kill()
            throw 'psmux process timed out after 30 seconds'
        }
        [pscustomobject]@{
            Exit = $Run.Process.ExitCode
            Output = if ($Run.Out) { $Run.Out.GetAwaiter().GetResult().Trim() } else { '' }
            Error = if ($Run.Err) { $Run.Err.GetAwaiter().GetResult().Trim() } else { '' }
        }
    } finally { $Run.Process.Dispose() }
}
function Invoke-Psmux([string[]]$Arguments) { Complete-Psmux (Start-Psmux $Arguments) }
function P([string]$Namespace, [string[]]$Arguments) {
    $r = Invoke-Psmux (@('-L', $Namespace, '-f', $config) + $Arguments)
    if ($r.Exit -ne 0) { throw "psmux $Namespace $Arguments failed ($($r.Exit)): $($r.Output) $($r.Error)" }
    $r.Output
}
function Outer { P $outerNS $args }
function Control { P $controlNS $args }
function Default-Snapshot {
    $r = Invoke-Psmux @('-f', $config, 'list-sessions', '-F', '#{session_id}:#{session_name}')
    if ($r.Exit -ne 0) {
        if ("$($r.Output) $($r.Error)" -match 'no server|no sessions') { return '<no sessions>' }
        throw "default snapshot failed: $($r.Output) $($r.Error)"
    }
    ($r.Output -split '\r?\n' | Sort-Object) -join "`n"
}
function Wait-For([scriptblock]$Condition, [string]$Description) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out waiting for $Description"
}
function Literal([string]$Value) { "'" + $Value.Replace("'", "''") + "'" }
function Send-Line([string]$Pane, [string]$Line) {
    Outer send-keys -t $Pane -l -- $Line | Out-Null
    Outer send-keys -t $Pane Enter | Out-Null
}

# Console API calls are injected only into the owned ConPTY child.
$childHelpers = @'
Add-Type @"
using System.Runtime.InteropServices;
public static class ConsoleCP {
 [DllImport("kernel32.dll")] public static extern uint GetConsoleCP();
 [DllImport("kernel32.dll")] public static extern uint GetConsoleOutputCP();
 [DllImport("kernel32.dll")] public static extern bool SetConsoleCP(uint cp);
 [DllImport("kernel32.dll")] public static extern bool SetConsoleOutputCP(uint cp);
}
"@
function Save-Status($Value) {
    [IO.File]::WriteAllText($statusPath + '.tmp', ($Value | ConvertTo-Json -Compress))
    [IO.File]::Move($statusPath + '.tmp', $statusPath)
}
'@
$childHelpers += "`nfunction Start-Psmux { ${function:Start-Psmux} }"
$childHelpers += "`nfunction Complete-Psmux { ${function:Complete-Psmux} }"
function New-Subject([string]$Body, [string]$StatusPath, [int]$CodePage = 437) {
    $code = '$ErrorActionPreference = ''Stop''' + "`n" + $childHelpers + "`n" + @"
`$PsmuxExe = $(Literal $PsmuxExe)
`$statusPath = $(Literal $StatusPath)
try {
    if (-not [ConsoleCP]::SetConsoleCP($CodePage) -or -not [ConsoleCP]::SetConsoleOutputCP($CodePage)) { throw 'Cannot set child code pages' }
    [Console]::OutputEncoding = [Text.Encoding]::GetEncoding($CodePage)
    if ([Console]::IsInputRedirected -or [Console]::IsOutputRedirected) { throw 'Child requires console stdin and stdout' }
    $Body
} catch { Save-Status @{ Error = `$_ | Out-String } }
Start-Sleep -Seconds 120
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    Outer split-window -h -d -l 90 -t outer -P -F '#{pane_id}' -- pwsh.exe -NoLogo -NoProfile -EncodedCommand $encoded
}
function Read-Status([string]$Path) {
    Wait-For { Test-Path -LiteralPath $Path } "child metadata $Path"
    $s = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($s.Error) { throw "Child failed: $($s.Error)" }
    $s
}
function Close-Subject([string]$Pane, [string]$Label) {
    if ($Pane) {
        try { Outer kill-pane -t $Pane | Out-Null }
        catch { $failures.Add("$Label cleanup: $_") }
    }
}

try {
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    . "$PSScriptRoot/isolated_config.ps1"
    $config = $PsmuxIsolatedConfig
    $defaultBefore = Default-Snapshot
    $env:PSMUX_DATA_DIR = Join-Path $tempDir 'state'
    $env:PSMUX_ALLOW_NESTING = '1'
    $privateRoot = $true
    Outer new-session -d -s outer -x 140 -y 40 | Out-Null
    Control new-session -d -s inner -x 80 -y 24 | Out-Null

    foreach ($terminal in @($false, $true)) {
        $label = "query terminal=$terminal"
        $pane = $null
        $status = Join-Path $tempDir "query-$terminal.json"
        $query = 'QUERY:' + $unicode + ':END'
        $arguments = (@('-L', $outerNS, '-f', $config, 'display-message', '-p', '-t', 'outer', $query) | ForEach-Object { Literal $_ }) -join ','
        $body = @"
[Console]::WriteLine('BEFORE:' + [char]0x2588 + ':END')
`$beforeIn = [ConsoleCP]::GetConsoleCP(); `$beforeOut = [ConsoleCP]::GetConsoleOutputCP()
`$r = Complete-Psmux (Start-Psmux @($arguments) -ShareConsole -Terminal:`$$terminal)
`$afterIn = [ConsoleCP]::GetConsoleCP(); `$afterOut = [ConsoleCP]::GetConsoleOutputCP()
[Console]::WriteLine('AFTER:' + [char]0x2588 + ':END')
Save-Status @{ BeforeInput = `$beforeIn; BeforeOutput = `$beforeOut; AfterInput = `$afterIn; AfterOutput = `$afterOut; Exit = `$r.Exit; Query = `$r.Output; ErrorText = `$r.Error }
"@
        try {
            $pane = New-Subject $body $status
            $s = Read-Status $status
            Wait-For { (Outer capture-pane -p -t $pane).Contains('AFTER:') } $label
            $capture = Outer capture-pane -p -t $pane
            if ($s.BeforeInput -ne 437 -or $s.BeforeOutput -ne 437 -or -not $capture.Contains("BEFORE:${block}:END")) {
                throw "CP437 cached-writer precondition failed: $($s | ConvertTo-Json -Compress); $capture"
            }
            if ($s.Exit -ne 0 -or ($terminal -and -not $capture.Contains($query)) -or (-not $terminal -and $s.Query -cne $query) -or
                $s.AfterInput -ne 437 -or $s.AfterOutput -ne 437 -or -not $capture.Contains("AFTER:${block}:END")) {
                throw "Console/query regression: $($s | ConvertTo-Json -Compress); capture: $capture"
            }
            Write-Host "PASS $label"
        } catch { $failures.Add("${label}: $_") }
        finally { Close-Subject $pane $label }
    }

    # Child status and redirected bytes survive non-UTF-8 stdout and stderr.
    foreach ($stream in @('stdout', 'stderr')) {
        foreach ($terminal in @($false, $true)) {
            $label = "child bytes $stream terminal=$terminal"
            $pane = $null
            $status = Join-Path $tempDir "bytes-$stream-$terminal.json"
            $method = if ($stream -eq 'stdout') { 'OpenStandardOutput' } else { 'OpenStandardError' }
            $payload = 'CHILD:' + $unicode + ':END'
            $raw = [byte[]](@(255, 10) + [Text.Encoding]::UTF8.GetBytes($payload + "`n"))
            $expectedBytes = [Convert]::ToBase64String($raw)
            $emitCode = "`$data=[Convert]::FromBase64String('$expectedBytes'); `$stream=[Console]::$method(); `$stream.Write(`$data,0,`$data.Length); exit 17"
            $emit = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($emitCode))
            $arguments = (@('-L', $outerNS, '-f', $config, 'run-shell', "pwsh.exe -NoProfile -EncodedCommand $emit") | ForEach-Object { Literal $_ }) -join ','
            $body = @"
`$si = [Diagnostics.ProcessStartInfo]::new(`$PsmuxExe)
`$si.UseShellExecute = `$false; `$si.CreateNoWindow = `$false
`$si.RedirectStandardOutput = -not `$$terminal; `$si.RedirectStandardError = -not `$$terminal
foreach (`$a in @($arguments)) { [void]`$si.ArgumentList.Add(`$a) }
`$p = [Diagnostics.Process]::Start(`$si)
`$out = [IO.MemoryStream]::new(); `$err = [IO.MemoryStream]::new()
try {
    if (-not `$$terminal) { `$o = `$p.StandardOutput.BaseStream.CopyToAsync(`$out); `$e = `$p.StandardError.BaseStream.CopyToAsync(`$err) }
    if (-not `$p.WaitForExit(30000)) { `$p.Kill(); throw 'Raw output child timed out' }
    if (-not `$$terminal) { `$o.GetAwaiter().GetResult(); `$e.GetAwaiter().GetResult() }
    Save-Status @{ Exit = `$p.ExitCode; Out = [Convert]::ToBase64String(`$out.ToArray()); Err = [Convert]::ToBase64String(`$err.ToArray()) }
} finally { `$p.Dispose(); `$out.Dispose(); `$err.Dispose() }
"@
            try {
                $pane = New-Subject $body $status
                $s = Read-Status $status
                $data = if ($stream -eq 'stdout') { $s.Out } else { $s.Err }
                $other = if ($stream -eq 'stdout') { $s.Err } else { $s.Out }
                if ($s.Exit -ne 17 -or (-not $terminal -and ($data -cne $expectedBytes -or $other))) {
                    throw "Child output/status regression: $($s | ConvertTo-Json -Compress)"
                }
                if ($terminal) { Wait-For { (Outer capture-pane -p -t $pane).Contains($payload) } "$label Unicode child output" }
                Write-Host "PASS $label"
            } catch { $failures.Add("${label}: $_") }
            finally { Close-Subject $pane $label }
        }
    }

    foreach ($cp in @(437, 1252, 65001)) {
        foreach ($mode in @('-C', '-CC')) {
            foreach ($terminal in @($false, $true)) {
                $label = "control cp=$cp $mode terminal=$terminal"
                $pane = $null
                $status = Join-Path $tempDir "control-$cp-$mode-$terminal.json"
                $arguments = (@('-L', $controlNS, '-f', $config, $mode, 'attach-session', '-t', 'inner') | ForEach-Object { Literal $_ }) -join ','
                $body = @"
Save-Status @{ InputCP = [ConsoleCP]::GetConsoleCP(); OutputCP = [ConsoleCP]::GetConsoleOutputCP() }
`$run = Start-Psmux @($arguments) -ShareConsole -Terminal:`$$terminal
try { `$run.Process.WaitForExit() } finally { `$run.Process.Dispose() }
"@
                try {
                    Control set-option -g @unicode-probe unset | Out-Null
                    Control set-option -g @unicode-barrier unset | Out-Null
                    if ((Control display-message -p -t inner '#{@unicode-probe}') -cne 'unset') { throw 'precondition: probe not reset' }
                    if ((Control list-clients)) { throw 'precondition: previous control client remains' }
                    $pane = New-Subject $body $status $cp
                    $s = Read-Status $status
                    if ($s.InputCP -ne $cp -or $s.OutputCP -ne $cp) { throw "Child CP precondition failed: $($s | ConvertTo-Json -Compress)" }
                    Wait-For { [bool](Control list-clients) } "$label registration"
                    Send-Line $pane 'set-option -g @unicode-probe ascii-sentinel'
                    Wait-For { (Control display-message -p -t inner '#{@unicode-probe}') -ceq 'ascii-sentinel' } "$label ASCII precondition"
                    if ((Control display-message -p -t inner '#{@unicode-barrier}') -cne 'unset') { throw 'precondition: barrier already set' }
                    Send-Line $pane ("set-option -g @unicode-probe '$unicode'")
                    Send-Line $pane 'set-option -g @unicode-barrier done'
                    Wait-For {
                        (Control display-message -p -t inner '#{@unicode-barrier}') -ceq 'done' -or -not (Control list-clients)
                    } "$label Unicode/barrier"
                    $value = Control display-message -p -t inner '#{@unicode-probe}'
                    $barrier = Control display-message -p -t inner '#{@unicode-barrier}'
                    $clients = Control list-clients
                    if ($value -cne $unicode -or $barrier -cne 'done' -or -not $clients) {
                        throw "Expected '$unicode', received '$value'; barrier='$barrier'; clients='$clients'"
                    }
                    Outer send-keys -t $pane -l -- "set-option -g @unicode-probe 'ctrl" | Out-Null
                    Outer send-keys -t $pane C-z | Out-Null
                    Send-Line $pane "z'"
                    Send-Line $pane 'set-option -g @unicode-barrier ctrl-z-done'
                    Wait-For {
                        (Control display-message -p -t inner '#{@unicode-barrier}') -ceq 'ctrl-z-done' -or -not (Control list-clients)
                    } "$label raw Ctrl+Z"
                    if ((Control display-message -p -t inner '#{@unicode-barrier}') -cne 'ctrl-z-done' -or -not (Control list-clients)) {
                        throw 'Raw Ctrl+Z terminated the control transport'
                    }
                    Write-Host "PASS $label"
                } catch {
                    $failures.Add("${label}: $_")
                    if ($pane) {
                        try { $failures.Add("${label} capture: $(Outer capture-pane -p -t $pane)") }
                        catch { $failures.Add("${label} capture failed: $_") }
                    }
                } finally { Close-Subject $pane $label }
                Wait-For { -not (Control list-clients) } "$label disconnect cleanup"
            }
        }
    }

    # A real UTF-8 pipe must still process its final line and disconnect at EOF.
    foreach ($mode in @('-C', '-CC')) {
        $label = "UTF8 pipe $mode EOF"
        Control set-option -g @unicode-probe unset | Out-Null
        Control set-option -g @unicode-barrier unset | Out-Null
        if ((Control display-message -p -t inner '#{@unicode-probe}') -cne 'unset' -or (Control list-clients)) {
            throw 'precondition: pipe probe not reset or previous client remains'
        }
        $run = Start-Psmux @('-L', $controlNS, '-f', $config, $mode, 'attach-session', '-t', 'inner') -PipeInput
        try {
            Wait-For { [bool](Control list-clients) } "$label registration"
            $run.Process.StandardInput.WriteLine('set-option -g @unicode-probe ascii-sentinel')
            $run.Process.StandardInput.Flush()
            Wait-For { (Control display-message -p -t inner '#{@unicode-probe}') -ceq 'ascii-sentinel' } "$label ASCII precondition"
            if ((Control display-message -p -t inner '#{@unicode-barrier}') -cne 'unset') { throw 'precondition: barrier already set' }
            $run.Process.StandardInput.WriteLine("set-option -g @unicode-probe '$unicode'")
            $run.Process.StandardInput.WriteLine('set-option -g @unicode-barrier done')
            $run.Process.StandardInput.Flush()
            Wait-For { (Control display-message -p -t inner '#{@unicode-barrier}') -ceq 'done' } "$label barrier before EOF"
            if ((Control display-message -p -t inner '#{@unicode-probe}') -cne $unicode -or -not (Control list-clients)) {
                throw 'UTF8 pipe corrupted Unicode or disconnected before EOF'
            }
            Control set-option -g @unicode-at-eof unset | Out-Null
            if ((Control display-message -p -t inner '#{@unicode-at-eof}') -cne 'unset') { throw 'precondition: EOF probe not reset' }
            $run.Process.StandardInput.WriteLine('set-option -g @unicode-at-eof final')
            $run.Process.StandardInput.Close()
            $finishing = $run
            $run = $null
            $r = Complete-Psmux $finishing
            $value = Control display-message -p -t inner '#{@unicode-probe}'
            $barrier = Control display-message -p -t inner '#{@unicode-barrier}'
            if ($r.Exit -ne 0 -or $value -cne $unicode -or $barrier -cne 'done' -or
                (Control display-message -p -t inner '#{@unicode-at-eof}') -cne 'final') {
                throw "exit=$($r.Exit); value='$value'; barrier='$barrier'; $($r.Output) $($r.Error)"
            }
            Wait-For { -not (Control list-clients) } "$label disconnect"
            Write-Host "PASS $label"
        } catch { $failures.Add("${label}: $_") }
        finally {
            if ($run) {
                try { if (-not $run.Process.HasExited) { $run.Process.Kill() } }
                finally { $run.Process.Dispose() }
            }
        }
    }

    # mintty, Git Bash and Cygwin sshd hand a native program a pipe named like an
    # MSYS2 pty. Rust's is_terminal() is true for it, yet it is no console, so
    # ReadConsoleW must not be used: control mode has to read it as a pipe.
    foreach ($mode in @('-C', '-CC')) {
        $label = "MSYS pty pipe $mode"
        $server = $null; $writer = $null; $proc = $null
        try {
            Control set-option -g @unicode-probe unset | Out-Null
            if ((Control list-clients)) { throw 'precondition: previous control client remains' }
            $pipeName = 'msys-1888ae32e00d56aa-pty0-from-master-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
            $server = [IO.Pipes.NamedPipeServerStream]::new($pipeName, [IO.Pipes.PipeDirection]::Out)
            $si = [Diagnostics.ProcessStartInfo]::new('cmd.exe')
            $si.Arguments = "/s /c `"`"$PsmuxExe`" -L $controlNS -f `"$config`" $mode attach-session -t inner < \\.\pipe\$pipeName > nul 2>&1`""
            $si.UseShellExecute = $false; $si.CreateNoWindow = $true
            $proc = [Diagnostics.Process]::Start($si)
            if (-not $server.WaitForConnectionAsync().Wait(15000)) { throw 'control client never opened the pipe' }
            $writer = [IO.StreamWriter]::new($server, [Text.UTF8Encoding]::new($false))
            Wait-For { [bool](Control list-clients) } "$label registration"
            $writer.Write("set-option -g @unicode-probe '$unicode'`n"); $writer.Flush()
            Wait-For { (Control display-message -p -t inner '#{@unicode-probe}') -ceq $unicode -or $proc.HasExited } "$label Unicode"
            if ((Control display-message -p -t inner '#{@unicode-probe}') -cne $unicode) {
                throw "MSYS pty pipe input was not read (client exited=$($proc.HasExited))"
            }
            $writer.Dispose(); $writer = $null
            $server.Dispose(); $server = $null
            if (-not $proc.WaitForExit(15000)) { throw 'control client did not exit at pipe EOF' }
            Wait-For { -not (Control list-clients) } "$label disconnect"
            Write-Host "PASS $label"
        } catch { $failures.Add("${label}: $_") }
        finally {
            if ($writer) { try { $writer.Dispose() } catch {} }
            if ($server) { $server.Dispose() }
            if ($proc) {
                if (-not $proc.HasExited) { try { $proc.Kill($true) } catch {} }
                $proc.Dispose()
            }
        }
    }
} catch { $failures.Add(($_ | Out-String)) }
finally {
    if ($privateRoot) {
        foreach ($ns in @($controlNS, $outerNS)) {
            try {
                $r = Invoke-Psmux @('-L', $ns, '-f', $config, 'kill-server')
                if ($r.Exit -ne 0 -and "$($r.Output) $($r.Error)" -notmatch 'no server|no sessions') {
                    throw "$($r.Output) $($r.Error)"
                }
            } catch { $failures.Add("cleanup $ns : $_") }
        }
    }
    $env:PSMUX_DATA_DIR = $previousRoot
    $env:PSMUX_ALLOW_NESTING = $previousNesting
    try {
        if ($null -ne $defaultBefore -and (Default-Snapshot) -cne $defaultBefore) { throw 'Default namespace sessions changed' }
    } catch { $failures.Add(($_ | Out-String)) }
    try { if ($config) { Remove-PsmuxIsolatedConfig } }
    catch { $failures.Add("config cleanup: $_") }
    try { if (Test-Path -LiteralPath $tempDir) { Remove-Item -LiteralPath $tempDir -Recurse -Force } }
    catch { $failures.Add("temp cleanup: $_") }
}
if ($failures.Count) { throw ($failures -join "`n") }
