# Issue #919: input-buffer-size, the tmux server option that bounds one input
# string (options-table.c:434, default and minimum 1048576, maximum
# 4294967295), exists, reads and refuses values as tmux 3.7c does, and sets
# how large a sixel DCS a pane may write before psmux drops it.
#
#   [A] the option: show -gv default, set and show, out of range values
#       refused with tmux's wording and a failing exit code, set -gu, and a
#       config line through source-file.
#   [B] the limit: a sixel of about 1.5 MiB is dropped under the default,
#       kept once the option is 4 MiB, and dropped again after set -gu.
#       Needs PSMUX_CONPTY_DIR_FOR_TEST, a directory holding conpty.dll and
#       OpenConsole.exe (the inbox conhost strips sixel); without it [B] is
#       reported as SKIP.
#
# Isolation: private PSMUX_DATA_DIR and empty config, unique -L namespace on
# every command, cleanup only with `-L <ns> kill-server`.

$ErrorActionPreference = 'Continue'

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$ns = "tibs_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_ibs_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$oldData = $env:PSMUX_DATA_DIR
$oldConf = $env:PSMUX_CONFIG_FILE
$oldConpty = $env:PSMUX_CONPTY_DIR
$env:PSMUX_DATA_DIR = $data
$emptyConf = Join-Path $data 'empty.conf'
Set-Content -Path $emptyConf -Value '' -Encoding UTF8
$env:PSMUX_CONFIG_FILE = $emptyConf
Remove-Item Env:\TMUX -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX_PANE -ErrorAction SilentlyContinue

$conptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
$haveConpty = $conptyDir -and (Test-Path (Join-Path $conptyDir 'conpty.dll')) -and (Test-Path (Join-Path $conptyDir 'OpenConsole.exe'))
if ($haveConpty) { $env:PSMUX_CONPTY_DIR = $conptyDir } else { Remove-Item Env:\PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:TestsPassed++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:TestsFailed++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>$null }
# Runs a command and returns its exit code and everything it printed.
function PErr {
    $text = (& $exe -L $ns @args 2>&1 | ForEach-Object { "$_" }) -join "`n"
    return @{ Code = $LASTEXITCODE; Text = $text }
}

function WaitSession($s) {
    for ($i = 0; $i -lt 60; $i++) {
        P has-session -t $s | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function WaitText($t, $text) {
    for ($i = 0; $i -lt 60; $i++) {
        if (((P capture-pane -t $t -p) -join "`n") -match $text) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# One dump-state frame over the session's TCP port, parsed.
function Get-Dump($s) {
    $port = (Get-Content (Join-Path $data "${ns}__$s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $data "${ns}__$s.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    try {
        $tcp.ReceiveTimeout = 4000
        $st = $tcp.GetStream()
        $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
        $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
        $w.Write("dump-state`n"); $w.Flush()
        for ($j = 0; $j -lt 300; $j++) {
            $line = $r.ReadLine()
            if ($null -eq $line) { break }
            if ($line -ne 'NC' -and $line.StartsWith('{')) { return ($line | ConvertFrom-Json -AsHashtable) }
        }
    } catch {
    } finally {
        $tcp.Close()
    }
    return $null
}

function Get-Leaves($node) {
    if ($null -eq $node) { return @() }
    if ($node['type'] -eq 'leaf') { return @(, $node) }
    $out = @()
    foreach ($c in $node['children']) { $out += Get-Leaves $c }
    return $out
}

# Writes a sixel of about 1.5 MiB in a new window and returns how many images
# that window holds. The padding is `#1` colour selects, which draw nothing,
# so the image stays 4 by 6 pixels however long the DCS is.
function Emit-BigSixel($mark) {
    P new-window -t a 'pwsh -NoProfile -NoLogo' | Out-Null
    Start-Sleep -Seconds 3
    $cmd = "`$p = '#1' * 786432; [Console]::Out.Write([char]27 + 'Pq#1;2;100;100;0#1~~' + `$p + '~~' + [char]27 + '\'); '$mark'"
    P send-keys -t a -l $cmd | Out-Null
    P send-keys -t a Enter | Out-Null
    $null = WaitText 'a' "(?m)^$mark\s*$"
    Start-Sleep -Milliseconds 800
    P refresh-client -t a | Out-Null
    $d = Get-Dump 'a'
    if ($null -eq $d) { return -1 }
    return @((Get-Leaves $d['layout']) | ForEach-Object { $_['images'] } | Where-Object { $_ }).Count
}

try {
    Write-Host "`n=== input-buffer-size ($exe) ===" -ForegroundColor Cyan
    P new-session -d -s a -x 100 -y 30 'pwsh -NoProfile -NoLogo' | Out-Null
    if (-not (WaitSession 'a')) { Check 'session starts' $false ''; throw 'no session' }

    Write-Host "`n[A] the option" -ForegroundColor Yellow
    $v = (P show -gv input-buffer-size) -join ''
    Check 'the default is 1048576' ($v -eq '1048576') "got '$v'"
    $listed = @(P show -s) -match '^input-buffer-size '
    Check 'show -s lists it' ($listed.Count -eq 1) "got '$($listed -join '|')'"

    $r = PErr set -g input-buffer-size 2097152
    $v = (P show -gv input-buffer-size) -join ''
    Check 'set 2097152 is accepted' ($r.Code -eq 0 -and $v -eq '2097152') "code=$($r.Code) got '$v'"

    foreach ($case in @(
        @('1048575', 'value is too small: 1048575'),
        @('100', 'value is too small: 100'),
        @('4294967296', 'value is too large: 4294967296'))) {
        $r = PErr set -g input-buffer-size $case[0]
        $v = (P show -gv input-buffer-size) -join ''
        Check "set $($case[0]) is refused as tmux refuses it" ($r.Code -ne 0 -and $r.Text -match [regex]::Escape($case[1]) -and $v -eq '2097152') "code=$($r.Code) text='$($r.Text)' value='$v'"
    }

    $r = PErr set -g input-buffer-size 4294967295
    $v = (P show -gv input-buffer-size) -join ''
    Check 'the maximum 4294967295 is accepted' ($r.Code -eq 0 -and $v -eq '4294967295') "code=$($r.Code) got '$v'"

    P set -gu input-buffer-size | Out-Null
    $v = (P show -gv input-buffer-size) -join ''
    Check 'set -gu restores 1048576' ($v -eq '1048576') "got '$v'"

    $conf = Join-Path $data 'ibs.conf'
    Set-Content -Path $conf -Value 'set -g input-buffer-size 3145728' -Encoding UTF8
    P source-file $conf | Out-Null
    $v = (P show -gv input-buffer-size) -join ''
    Check 'a config line sets it' ($v -eq '3145728') "got '$v'"
    P set -gu input-buffer-size | Out-Null

    Write-Host "`n[B] the sixel limit" -ForegroundColor Yellow
    if (-not $haveConpty) {
        Write-Host "  SKIP: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe" -ForegroundColor Yellow
    } else {
        $n = Emit-BigSixel 'IBS_DEFAULT'
        Check 'a 1.5 MiB sixel is dropped under the default' ($n -eq 0) "images=$n"

        P set -g input-buffer-size 4194304 | Out-Null
        $n = Emit-BigSixel 'IBS_RAISED'
        Check 'the same sixel is kept with input-buffer-size 4194304' ($n -eq 1) "images=$n"

        P set -gu input-buffer-size | Out-Null
        $n = Emit-BigSixel 'IBS_UNSET'
        Check 'it is dropped again after set -gu' ($n -eq 0) "images=$n"
    }
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    $env:PSMUX_DATA_DIR = $oldData
    $env:PSMUX_CONFIG_FILE = $oldConf
    if ($oldConpty) { $env:PSMUX_CONPTY_DIR = $oldConpty } else { Remove-Item Env:\PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
exit $script:TestsFailed
