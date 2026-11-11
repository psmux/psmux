# Listing commands end in exactly one newline, as tmux prints them.
#
# list-panes (default format), list-panes -s, list-buffers, list-keys,
# list-commands, show-options and show-hooks printed an empty last line: the
# server wrote `{}\n` after text whose rows already ended in a newline, so a
# script counting `(psmux list-panes).Count` got panes + 1.
#
# Checks the raw stdout bytes of the CLI and the raw reply over TCP, for each
# listing with and without -F, -a, -s and -t.
param([string]$Exe)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

if (-not $Exe) {
    $cand = Join-Path $PSScriptRoot '..\target\release\psmux.exe'
    $Exe = if (Test-Path $cand) { (Resolve-Path $cand).Path } else { (Get-Command psmux -ErrorAction Stop).Source }
}
$ns = "tltn" + (Get-Random -Maximum 999999)
$dataDir = Join-Path $env:TEMP ("psmux_tltn_" + (Get-Random))
New-Item -ItemType Directory -Force $dataDir | Out-Null
$env:PSMUX_DATA_DIR = $dataDir

$pass = 0; $fail = 0
function Check([bool]$ok, [string]$name, [string]$detail = '') {
    if ($ok) { $script:pass++; Write-Host "  PASS $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "  FAIL $name $detail" -ForegroundColor Red }
}

function RawCli([string[]]$a) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    foreach ($x in (@('-L', $ns) + $a)) { [void]$psi.ArgumentList.Add($x) }
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false
    $p = [System.Diagnostics.Process]::Start($psi)
    $ms = New-Object System.IO.MemoryStream
    $p.StandardOutput.BaseStream.CopyTo($ms)
    $null = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    [System.Text.Encoding]::UTF8.GetString($ms.ToArray())
}

function RawTcp([string]$line) {
    $pf = Get-ChildItem $dataDir -Recurse -Filter "${ns}__a.port" | Select-Object -First 1
    $kf = Get-ChildItem $dataDir -Recurse -Filter "${ns}__a.key" | Select-Object -First 1
    $port = [int](Get-Content $pf.FullName -Raw).Trim()
    $key = (Get-Content $kf.FullName -Raw).Trim()
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect('127.0.0.1', $port)
    $s = $tcp.GetStream(); $s.ReadTimeout = 5000
    $req = [System.Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
    $s.Write($req, 0, $req.Length); $s.Flush()
    $ms = New-Object System.IO.MemoryStream
    $buf = New-Object byte[] 65536
    try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $tcp.Close()
    $t = [System.Text.Encoding]::UTF8.GetString($ms.ToArray())
    # Drop the AUTH ack line.
    if ($t.StartsWith("OK`n")) { $t.Substring(3) } else { $t }
}

function OneNewline([string]$text, [string]$name) {
    $ok = $text.Length -gt 0 -and $text.EndsWith("`n") -and -not $text.EndsWith("`n`n") -and -not $text.EndsWith("`r`n`r`n")
    $tail = (($text.ToCharArray() | Select-Object -Last 6 | ForEach-Object { '{0:x2}' -f [int]$_ }) -join ' ')
    Check $ok $name "(tail bytes: $tail)"
}

try {
    & $Exe -L $ns new-session -d -s a -x 120 -y 30
    Start-Sleep -Milliseconds 1500
    & $Exe -L $ns split-window -t a
    & $Exe -L $ns new-window -t a
    & $Exe -L $ns split-window -t a
    & $Exe -L $ns set-buffer hello
    & $Exe -L $ns set-hook -g after-new-window 'display-message hi'
    Start-Sleep -Milliseconds 800

    Write-Host "CLI raw stdout ends in exactly one newline"
    $cases = @(
        @('list-panes'), @('list-panes', '-t', 'a'), @('list-panes', '-t', 'a:0'), @('list-panes', '-s', '-t', 'a'),
        @('list-panes', '-a'), @('list-panes', '-F', '#{pane_id}'), @('list-panes', '-a', '-F', '#{pane_id}'),
        @('list-panes', '-s', '-F', '#{pane_id}'),
        @('list-windows'), @('list-windows', '-a'), @('list-windows', '-F', '#{window_id}'),
        @('list-sessions'), @('list-sessions', '-F', '#{session_name}'),
        @('list-buffers'), @('list-buffers', '-F', '#{buffer_name}'),
        @('list-keys'), @('list-keys', '-T', 'prefix'), @('list-commands'),
        @('show-options'), @('show-options', '-g'), @('show-options', '-gv', 'status'),
        @('show-hooks', '-g'), @('show-environment'), @('show-environment', '-g'),
        @('display-message', '-p', '#{session_name}'), @('capture-pane', '-p'), @('show-messages')
    )
    foreach ($c in $cases) { OneNewline (RawCli $c) ("cli: " + ($c -join ' ')) }
    # A window that owns no options prints nothing at all (#655).
    Check ((RawCli @('show-options', '-w')) -eq '') "cli: show-options -w with nothing set prints nothing"

    Write-Host "Line counts match the pane count (the reported symptom)"
    $n0 = (& $Exe -L $ns list-panes -t a:0 -F '#{pane_id}').Count
    Check ((RawCli @('list-panes', '-t', 'a:0')) -split "`n" | Where-Object { $_ -ne '' }).Count -eq $n0 "list-panes rows = $n0"
    Check (([regex]::Matches((RawCli @('list-panes', '-t', 'a:0')), "`n")).Count -eq 2) "list-panes -t a:0 prints 2 newlines for 2 panes"
    Check (([regex]::Matches((RawCli @('list-panes', '-s', '-t', 'a')), "`n")).Count -eq 4) "list-panes -s prints 4 newlines for 4 panes"
    Check (([regex]::Matches((RawCli @('list-buffers')), "`n")).Count -eq 1) "list-buffers prints 1 newline for 1 buffer"

    Write-Host "Raw TCP reply ends in exactly one newline"
    foreach ($l in @('list-panes', 'list-panes -s', 'list-panes -F "#{pane_id}"', 'list-windows', 'list-buffers',
                     'list-keys', 'list-commands', 'show-options -g', 'show-hooks -g', 'show-messages')) {
        OneNewline (RawTcp $l) "tcp: $l"
    }
    Check ((RawTcp 'list-panes -t a:0') -eq (RawCli @('list-panes', '-t', 'a:0'))) "tcp and cli list-panes agree"
}
finally {
    & $Exe -L $ns kill-server 2>$null
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Passed: $pass  Failed: $fail"
if ($fail -gt 0) { exit 1 } else { exit 0 }
