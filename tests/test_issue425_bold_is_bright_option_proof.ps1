# Issue #425 follow-up: `bold-is-bright` option ground-truth proof.
#
# The 77f0dc9 fix rewrites crossterm's 256-indexed `38;5;N` (N<=15) to standard
# SGR so WT applies "bold is bright" to the 16 basic colors.  Side effect
# (reported by dkaszews): an EXPLICIT 256-indexed low color + bold, ESC[38;5;2;1m,
# also gets brightened, diverging from outside psmux.  New option `bold-is-bright`
# (default on) lets a user opt out.
#
# This proves BOTH states at the byte level, hosting psmux under a real
# pseudoconsole exactly as WT does:
#   DEFAULT (on):  basic ESC[32;1m -> `32` (bright), 256 ESC[38;5;2;1m -> `32` (side effect)
#   OFF:           basic ESC[32;1m stays `38;5;2` (no bright), 256 ESC[38;5;2;1m stays `38;5;2` (accurate)
# So `off` makes psmux byte-for-byte match crossterm's raw output for BOTH forms.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
# The bytes below are read back through an inbox pseudo console, and on
# Windows 10 (build 19045) that host re-encodes every colour it renders: bold
# with a basic colour comes out as bold plus the bright code (1;92 for 1;32)
# and 38;5;N below 16 as the basic code, with no psmux in the path at all
# (the bare reference run turns 38;5;2;1 into 1;32 there). What psmux wrote
# cannot be read back on that host, so there is nothing to compare.
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    Write-Host "[SKIP] Windows 10 inbox pseudo console re-encodes the colours this suite compares (bold basic to bright, 38;5;N to basic)" -ForegroundColor Yellow
    exit 0
}
$HOSTEXE = "$env:TEMP\conpty_host.exe"
$CTRL = "$env:TEMP\conpty_ctrl.txt"
$OUT = "$env:TEMP\conpty_out.bin"
$CONF = "$env:TEMP\psmux_425_boldoff.conf"
$script:Pass = 0
$script:Fail = 0
function P($m){ Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function F($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }

if (-not (Test-Path $HOSTEXE)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize /out:$HOSTEXE (Join-Path $PSScriptRoot "conpty_ctrlc_host.cs") 2>&1 | Out-Null
}
if (-not (Test-Path $HOSTEXE)) { Write-Host "cannot compile conpty host" -F Red; exit 1 }

$ESC=0x1B; $LB=0x5B; $M=0x6D; $CR=0x0D; $LF=0x0A
function SgrLine($sgr,$tag){
  $l=New-Object System.Collections.Generic.List[byte]
  $l.Add($ESC);$l.Add($LB)
  foreach($b in [Text.Encoding]::ASCII.GetBytes($sgr)){$l.Add($b)}
  $l.Add($M)
  foreach($b in [Text.Encoding]::ASCII.GetBytes($tag)){$l.Add($b)}
  $l.Add($ESC);$l.Add($LB);$l.Add(0x30);$l.Add($M)
  $l.Add($CR);$l.Add($LF)
  return $l.ToArray()
}
$specs=[ordered]@{
  "BASICGRN" =@{ In="32;1" }        # basic green + bold
  "IDX256GRN"=@{ In="38;5;2;1" }    # explicit 256-indexed green + bold (side-effect case)
}
$all=New-Object System.Collections.Generic.List[byte]
foreach($k in $specs.Keys){ foreach($b in (SgrLine $specs[$k].In $k)){$all.Add($b)} }
[IO.File]::WriteAllBytes("$env:TEMP\clr425opt.bin",$all.ToArray())
$drv=@"
`$b=[IO.File]::ReadAllBytes('$env:TEMP\clr425opt.bin')
`$o=[Console]::OpenStandardOutput()
`$o.Write(`$b,0,`$b.Length);`$o.Flush()
"@
[IO.File]::WriteAllText("$env:TEMP\clr425opt_drv.ps1",$drv,(New-Object Text.UTF8Encoding($false)))

function Get-SgrBeforeMarkers($bytes){
  $hex=($bytes|ForEach-Object{$_.ToString("X2")}) -join ''
  $result=@{}
  foreach($tag in $specs.Keys){
    $thex=(([Text.Encoding]::ASCII.GetBytes($tag))|ForEach-Object{$_.ToString("X2")}) -join ''
    $idx=$hex.IndexOf($thex)
    if($idx -lt 0){ $result[$tag]=$null; continue }
    $start=[Math]::Max(0,($idx/2)-30)
    $seg=$bytes[$start..(($idx/2)-1)]
    $segStr=[Text.Encoding]::ASCII.GetString($seg)
    $codes=New-Object System.Collections.Generic.List[string]
    foreach($mrx in [regex]::Matches($segStr,"$([char]27)\[([0-9;]*)m")){
      foreach($p in ($mrx.Groups[1].Value -split ';')){ if($p -ne ''){ [void]$codes.Add($p) } }
    }
    $result[$tag]=$codes
  }
  return $result
}

function Run-Psmux($sessionArgs){
  & $PSMUX kill-session -t clr425o 2>&1 | Out-Null
  Start-Sleep -Milliseconds 400
  [IO.File]::Delete($CTRL); [IO.File]::Delete($OUT)
  $p = Start-Process -FilePath $HOSTEXE -ArgumentList $sessionArgs -PassThru
  Start-Sleep -Seconds 5
  Add-Content $CTRL "TEXT & '$env:TEMP\clr425opt_drv.ps1'`n"
  Start-Sleep -Seconds 4
  Add-Content $CTRL "QUIT`n"
  Start-Sleep -Seconds 1
  & $PSMUX kill-session -t clr425o 2>&1 | Out-Null
  try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
  return [IO.File]::ReadAllBytes($OUT)
}

function Has($codes,$want){ return ($codes -contains $want) }
function HasIndexed($codes){ return (($codes -contains "38") -and ($codes -contains "5")) }

Write-Host "`n=== Issue #425: bold-is-bright OPTION ground-truth proof ===" -ForegroundColor Cyan
Write-Host "  psmux: $((& $PSMUX -V).Trim())"

# --- STATE 1: DEFAULT (option on) ---
Write-Host "`n--- STATE: default (bold-is-bright on) ---" -ForegroundColor Yellow
$defBytes = Run-Psmux @($PSMUX,"new-session","-s","clr425o")
$def = Get-SgrBeforeMarkers $defBytes
foreach($tag in $specs.Keys){ Write-Host ("    {0,-10}: {1}" -f $tag, ($(if($def[$tag]){$def[$tag] -join ','}else{'MISSING'}))) }

# basic green must be rewritten to standard 32 (bold-is-bright works)
if((Has $def["BASICGRN"] "32") -and -not (HasIndexed $def["BASICGRN"])){
  P "default: basic ESC[32;1m -> standard '32' (bold-is-bright active)"
} else { F "default: basic green not standard, got [$($def["BASICGRN"] -join ',')]" }
# 256-indexed green ALSO rewritten (the known side effect, present by default)
if((Has $def["IDX256GRN"] "32") -and -not (HasIndexed $def["IDX256GRN"])){
  P "default: explicit ESC[38;5;2;1m -> '32' (side effect present by default, as designed)"
} else { F "default: expected 256-indexed to be rewritten to 32, got [$($def["IDX256GRN"] -join ',')]" }

# --- STATE 2: bold-is-bright OFF via config ---
Write-Host "`n--- STATE: bold-is-bright off (opt-out) ---" -ForegroundColor Yellow
"set -g bold-is-bright off" | Set-Content -Path $CONF -Encoding UTF8
$env:PSMUX_CONFIG_FILE = $CONF
$offBytes = Run-Psmux @($PSMUX,"new-session","-s","clr425o")
$env:PSMUX_CONFIG_FILE = $null
$off = Get-SgrBeforeMarkers $offBytes
foreach($tag in $specs.Keys){ Write-Host ("    {0,-10}: {1}" -f $tag, ($(if($off[$tag]){$off[$tag] -join ','}else{'MISSING'}))) }

# With the option off, psmux passes crossterm output through untouched:
# BOTH forms stay 256-indexed 38;5;2 (byte-accurate; no rewrite).
if(HasIndexed $off["BASICGRN"] -and -not (Has $off["BASICGRN"] "32")){
  P "off: basic ESC[32;1m stays 256-indexed 38;5;2 (rewrite disabled)"
} else { F "off: basic green still rewritten, got [$($off["BASICGRN"] -join ',')]" }
if(HasIndexed $off["IDX256GRN"] -and -not (Has $off["IDX256GRN"] "32")){
  P "off: explicit ESC[38;5;2;1m stays 256-indexed 38;5;2 (SIDE EFFECT ELIMINATED, 256 accurate)"
} else { F "off: 256-indexed green still rewritten, got [$($off["IDX256GRN"] -join ',')]" }

# --- show-options reporting ---
Write-Host "`n--- show-options reporting ---" -ForegroundColor Yellow
& $PSMUX kill-session -t clr425q 2>&1 | Out-Null
& $PSMUX new-session -d -s clr425q 2>&1 | Out-Null
Start-Sleep -Seconds 3
$vOn = (& $PSMUX show-options -g -v bold-is-bright -t clr425q 2>&1 | Out-String).Trim()
if($vOn -eq "on"){ P "show-options default reports 'on'" } else { F "show-options default expected 'on', got '$vOn'" }
& $PSMUX set-option -g bold-is-bright off -t clr425q 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
$vOff = (& $PSMUX show-options -g -v bold-is-bright -t clr425q 2>&1 | Out-String).Trim()
if($vOff -eq "off"){ P "show-options after set-option reports 'off'" } else { F "show-options after set expected 'off', got '$vOff'" }
$fmt = (& $PSMUX display-message -t clr425q -p '#{bold-is-bright}' 2>&1 | Out-String).Trim()
if($fmt -eq "off"){ P "display-message #{bold-is-bright} reports 'off'" } else { F "display-message expected 'off', got '$fmt'" }
& $PSMUX kill-session -t clr425q 2>&1 | Out-Null

Remove-Item $CONF -Force -EA SilentlyContinue
Write-Host "`n=== Results: Passed=$($script:Pass) Failed=$($script:Fail) ===" -ForegroundColor Cyan
exit $script:Fail
