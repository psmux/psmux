# Issue #1051: `split-window -d -t A pwsh -NoProfile -Command ...` made the pane
# but exited 1 with "no reply from server": the CLI read -NoProfile in the
# shell command as -P and so expected a reply. tmux's getopt stops at the
# first word that is not an option, so the words of the command are never
# options. Uses its own -L namespace and PSMUX_DATA_DIR; honours PSMUX_TEST_BIN.
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$Bin = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
foreach ($Name in 'PSMUX_SERVER_MODE', 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION') {
    Remove-Item "Env:$Name" -ErrorAction SilentlyContinue
}
$Root = Join-Path $env:TEMP ('psmux1051-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory "$Root\data" | Out-Null
$env:PSMUX_DATA_DIR = "$Root\data"
$env:PSMUX_CONFIG_FILE = "$Root\empty.conf"
Set-Content $env:PSMUX_CONFIG_FILE ''
$Ns = 't1051' + (Get-Random -Maximum 999999)

$script:Pass = 0
$script:Fail = 0
function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
    if ($Ok) { $script:Pass++; Write-Host "  [PASS] $Name" -ForegroundColor Green }
    else { $script:Fail++; Write-Host "  [FAIL] $Name $Detail" -ForegroundColor Red }
}
function Invoke-Psmux { & $Bin -L $Ns @args }

# Runs one command, returns exit code, stderr text and the pane count after.
function Split-Case([string]$Session, [scriptblock]$Command) {
    $err = & $Command 2>&1 | Where-Object { $_ -is [System.Management.Automation.ErrorRecord] } | Out-String
    $code = $LASTEXITCODE
    $panes = 0
    for ($i = 0; $i -lt 30; $i++) {
        $panes = @(& $Bin -L $Ns list-panes -t $Session).Count
        if ($panes -ge 2) { break }
        Start-Sleep -Milliseconds 100
    }
    [pscustomobject]@{ Code = $code; Err = $err.Trim(); Panes = $panes }
}

try {
    $sessions = 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'R'
    foreach ($s in $sessions) { & $Bin -L $Ns new-session -d -s $s -x 120 -y 30 }
    Start-Sleep -Milliseconds 800

    Write-Host "Issue #1051: split-window with dash words in the command"
    $cases = @(
        @{ Name = 'no --, pwsh -NoProfile'; S = 'A'; C = { & $Bin -L $Ns split-window -d -t A pwsh -NoProfile -Command "Start-Sleep 30" } },
        @{ Name = 'with --'; S = 'B'; C = { & $Bin -L $Ns split-window -d -t B -- pwsh -NoProfile -Command "Start-Sleep 30" } },
        @{ Name = 'through @args (PowerShell drops the bare --)'; S = 'C'; C = { Invoke-Psmux split-window -d -t C -- pwsh -NoProfile -Command "Start-Sleep 30" } },
        @{ Name = 'pwsh -ExecutionPolicy'; S = 'D'; C = { & $Bin -L $Ns split-window -d -t D pwsh -ExecutionPolicy Bypass -Command "Start-Sleep 30" } },
        @{ Name = 'one quoted word after --'; S = 'E'; C = { & $Bin -L $Ns split-window -d -t E -- "pwsh -NoProfile -Command Start-Sleep 30" } },
        @{ Name = 'quoted word without --'; S = 'F'; C = { & $Bin -L $Ns split-window -d -t F "pwsh -NoProfile -Command Start-Sleep 30" } }
    )
    foreach ($c in $cases) {
        $r = Split-Case $c.S $c.C
        Check "$($c.Name): exit 0" ($r.Code -eq 0) "exit=$($r.Code)"
        Check "$($c.Name): no stderr" ($r.Err -eq '') "stderr=[$($r.Err)]"
        Check "$($c.Name): 2 panes" ($r.Panes -eq 2) "panes=$($r.Panes)"
    }

    # -P still prints the new pane and still requires a reply.
    $out = & $Bin -L $Ns split-window -d -P -F '#{pane_id}' -t G pwsh -NoProfile -Command "Start-Sleep 30" 2>&1
    $code = $LASTEXITCODE
    Check '-P prints the pane id' (($code -eq 0) -and ("$out" -match '^%\d+$')) "exit=$code out=[$out]"

    # The server reads the line itself (source-file, bindings, control mode):
    # every word after the options is the command, not only the first.
    function Find-Mark([string]$Mark) {
        for ($i = 0; $i -lt 50; $i++) {
            foreach ($id in (& $Bin -L $Ns list-panes -s -t G -F '#{session_name}:#{window_index}.#{pane_index}')) {
                if (((& $Bin -L $Ns capture-pane -p -t $id) -join ' ') -match $Mark) { return $true }
            }
            Start-Sleep -Milliseconds 200
        }
        $false
    }
    $conf = Join-Path $Root 'cmds.conf'
    Set-Content $conf @(
        'split-window -d pwsh -NoProfile -Command "Write-Output MARK_SRC_SPLIT; Start-Sleep 30"',
        'new-window -d cmd /c "echo MARK_SRC_NEWW & timeout 30"'
    )
    & $Bin -L $Ns source-file -t G $conf
    Check 'source-file: split-window runs every word' (Find-Mark 'MARK_SRC_SPLIT')
    Check 'source-file: new-window runs every word' (Find-Mark 'MARK_SRC_NEWW')

    # respawn-pane / respawn-window keep every word too (CLI and source-file).
    function Pane-Has([string]$Pane, [string]$Mark) {
        for ($i = 0; $i -lt 50; $i++) {
            if (((& $Bin -L $Ns capture-pane -p -t $Pane) -join ' ') -match $Mark) { return $true }
            Start-Sleep -Milliseconds 200
        }
        $false
    }
    $rp = 'R:0.0'
    & $Bin -L $Ns respawn-pane -k -t $rp pwsh -NoProfile -Command "Write-Output MARK_CLI_RESPAWN; Start-Sleep 30"
    Check "CLI respawn-pane: exit 0" ($LASTEXITCODE -eq 0)
    Check 'CLI respawn-pane runs every word' (Pane-Has $rp 'MARK_CLI_RESPAWN')
    Set-Content $conf "respawn-pane -k -t $rp pwsh -NoProfile -Command `"Write-Output MARK_SRC_RESPAWN; Start-Sleep 30`""
    & $Bin -L $Ns source-file -t R $conf
    Check 'source-file respawn-pane runs every word' (Pane-Has $rp 'MARK_SRC_RESPAWN')
    Set-Content $conf "respawn-window -k -t $rp cmd /c `"echo MARK_SRC_RESPAWNW & timeout 30`""
    & $Bin -L $Ns source-file -t R $conf
    Check 'source-file respawn-window runs every word' (Pane-Has $rp 'MARK_SRC_RESPAWNW')

    $psi = [System.Diagnostics.ProcessStartInfo]::new($Bin)
    foreach ($a in @('-L', $Ns, '-C', 'attach', '-t', 'G')) { $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.UseShellExecute = $false
    $cc = [System.Diagnostics.Process]::Start($psi)
    $null = $cc.StandardOutput.ReadToEndAsync()
    try {
        Start-Sleep -Milliseconds 800
        $cc.StandardInput.WriteLine('split-window -d pwsh -NoProfile -Command "Write-Output MARK_CC_SPLIT; Start-Sleep 30"')
        $cc.StandardInput.WriteLine('new-window -d pwsh -NoProfile -Command "Write-Output MARK_CC_NEWW; Start-Sleep 30"')
        $cc.StandardInput.Flush()
        Check 'control mode: split-window runs its command' (Find-Mark 'MARK_CC_SPLIT')
        Check 'control mode: new-window runs every word' (Find-Mark 'MARK_CC_NEWW')
        $cc.StandardInput.WriteLine("respawn-pane -k -t G:0.0 pwsh -NoProfile -Command `"Write-Output MARK_CC_RESPAWN; Start-Sleep 30`"")
        $cc.StandardInput.Flush()
        Check 'control mode: respawn-pane runs its command' (Pane-Has 'G:0.0' 'MARK_CC_RESPAWN')
        $cc.StandardInput.WriteLine("respawn-window -k -t G:0 pwsh -NoProfile -Command `"Write-Output MARK_CC_RESPAWNW; Start-Sleep 30`"")
        $cc.StandardInput.Flush()
        Check 'control mode: respawn-window runs every word' (Find-Mark 'MARK_CC_RESPAWNW')
    } finally {
        try { $cc.StandardInput.Close() } catch {}
        if (-not $cc.WaitForExit(3000)) { $cc.Kill() }
    }
} finally {
    & $Bin -L $Ns kill-server 2>$null
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Results: $script:Pass passed, $script:Fail failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
