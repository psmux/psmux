param([string]$Exe)
$ErrorActionPreference = "Continue"
$PSMUX = $Exe
$TempHome = Join-Path ([IO.Path]::GetTempPath()) ("p912_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome; $env:PSMUX_DATA_DIR = $TempHome
$tests = "C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-ad44d5289fdea1c2f\tests"
. "$tests\isolated_config.ps1" -Lines @(
    "bind T choose-tree -Zs `"set -g @picked '%%'`"",
    "bind U choose-tree -Zw `"set -g @picked '%%'`""
)
$NS = "p912_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
function P { & $PSMUX -L $NS @args 2>&1 }
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$INJ = Join-Path $TempHome "injector.exe"; $READ = Join-Path $TempHome "conread.exe"
& $csc /nologo /optimize /out:$INJ "$tests\injector.cs" | Out-Null
& $csc /nologo /optimize /out:$READ "$tests\conread.cs" | Out-Null
try {
    P new-session -d -s a912 -x 120 -y 30 | Out-Null
    P new-session -d -s b912 -x 120 -y 30 | Out-Null
    Start-Sleep 2
    $c = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach", "-t", "a912" -PassThru
    Start-Sleep 4
    & $INJ $c.Id "^b{SLEEP:500}T{SLEEP:1500}" | Out-Null
    Write-Host "--- session chooser screen ---"; (& $READ $c.Id) | Select-Object -First 12
    & $INJ $c.Id "{DOWN}{SLEEP:400}{ENTER}{SLEEP:1500}" | Out-Null
    Write-Host "after DOWN ENTER: @picked=[$(P show-options -gv '@picked')] client_session=[$(P list-clients -F '#{client_session}')]"
    & $INJ $c.Id "^b{SLEEP:500}U{SLEEP:1500}" | Out-Null
    Write-Host "--- tree screen ---"; (& $READ $c.Id) | Select-Object -First 12
    & $INJ $c.Id "{ENTER}{SLEEP:1500}" | Out-Null
    Write-Host "after ENTER: @picked=[$(P show-options -gv '@picked')] client_session=[$(P list-clients -F '#{client_session}')]"
    Stop-Process -Id $c.Id -Force -EA SilentlyContinue
} finally {
    P kill-server | Out-Null
    Remove-PsmuxIsolatedConfig
    Start-Sleep 1
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}
