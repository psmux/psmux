# Issue #425 SIDE EFFECT reproduction.
#
# After 77f0dc9, psmux rewrites 38;5;N (N<=15) -> standard 3N/9N so WT applies
# "bold is bright" to BASIC colors. Reporter dkaszews flagged a side effect:
# a program that EXPLICITLY emits a 256-indexed low color plus bold,
# ESC[38;5;2;1m, now renders bright in psmux but NOT outside psmux, because the
# rewrite cannot tell an explicit 256-indexed 0-15 apart from a basic color
# (crossterm collapses both to 38;5;N).
#
# This proves the divergence at the byte level:
#   REFERENCE (bare pwsh under ConPTY == WT direct): 38;5;2 + 1  (stays indexed)
#   PSMUX (after fix):                                32   + 1   (rewritten -> WT brightens)
# A divergence here == the side effect is REAL.

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
  $l.Add($ESC);$l.Add($LB);$l.Add(0x30);$l.Add($M)   # reset
  $l.Add($CR);$l.Add($LF)
  return $l.ToArray()
}
# tag -> incoming SGR
$specs=[ordered]@{
  "IDX256GRN"=@{ In="38;5;2;1" }   # EXPLICIT 256-indexed green + bold  (the side-effect case)
  "IDX256CYN"=@{ In="38;5;6;1" }   # EXPLICIT 256-indexed cyan  + bold
  "BASICGRN" =@{ In="32;1" }       # basic green + bold (the case the fix targets)
  "IDX256HI" =@{ In="38;5;120;1" } # 256-indexed 120 (>15) + bold -> must pass through untouched
}
$all=New-Object System.Collections.Generic.List[byte]
foreach($k in $specs.Keys){ foreach($b in (SgrLine $specs[$k].In $k)){$all.Add($b)} }
[IO.File]::WriteAllBytes("$env:TEMP\clr425sfx.bin",$all.ToArray())
$drv=@"
`$b=[IO.File]::ReadAllBytes('$env:TEMP\clr425sfx.bin')
`$o=[Console]::OpenStandardOutput()
`$o.Write(`$b,0,`$b.Length);`$o.Flush()
"@
[IO.File]::WriteAllText("$env:TEMP\clr425sfx_drv.ps1",$drv,(New-Object Text.UTF8Encoding($false)))

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

function Run-Child($label,$argList){
  & $PSMUX kill-session -t clr425s 2>&1 | Out-Null
  Start-Sleep -Milliseconds 400
  [IO.File]::Delete($CTRL); [IO.File]::Delete($OUT)
  $p = Start-Process -FilePath $HOSTEXE -ArgumentList $argList -PassThru
  Start-Sleep -Seconds $(if($label -match 'psmux'){5}else{3})
  Add-Content $CTRL "TEXT & '$env:TEMP\clr425sfx_drv.ps1'`n"
  Start-Sleep -Seconds 4
  Add-Content $CTRL "QUIT`n"
  Start-Sleep -Seconds 1
  & $PSMUX kill-session -t clr425s 2>&1 | Out-Null
  try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
  return [IO.File]::ReadAllBytes($OUT)
}

Write-Host "`n=== Issue #425 SIDE EFFECT: explicit 256-indexed low color + bold ===" -ForegroundColor Cyan
Write-Host "  psmux: $((& $PSMUX -V).Trim())"

$refBytes = Run-Child "reference" @("pwsh.exe","-NoLogo","-NoProfile")
$ref = Get-SgrBeforeMarkers $refBytes
Write-Host "`n--- REFERENCE (bare pwsh = WT direct) ---" -ForegroundColor Yellow
foreach($tag in $specs.Keys){ Write-Host ("    {0,-10}: {1}" -f $tag, ($(if($ref[$tag]){$ref[$tag] -join ','}else{'MISSING'}))) }

$psBytes = Run-Child "psmux" @($PSMUX,"new-session","-s","clr425s")
$ps = Get-SgrBeforeMarkers $psBytes
Write-Host "`n--- PSMUX render ---" -ForegroundColor Yellow
foreach($tag in $specs.Keys){ Write-Host ("    {0,-10}: {1}" -f $tag, ($(if($ps[$tag]){$ps[$tag] -join ','}else{'MISSING'}))) }

Write-Host "`n--- VERDICT ---" -ForegroundColor Cyan

# SIDE EFFECT PROOF: for the explicit 256-indexed low colors, psmux must DIVERGE
# from the reference (that divergence IS the reported side effect).
foreach($tag in @("IDX256GRN","IDX256CYN")){
  $refCodes = $(if($ref[$tag]){$ref[$tag] -join ','}else{'MISSING'})
  $psCodes  = $(if($ps[$tag]){$ps[$tag] -join ','}else{'MISSING'})
  $refIndexed = ($ref[$tag] -contains "38") -and ($ref[$tag] -contains "5")
  $psStandard = ($ps[$tag] -contains "32") -or ($ps[$tag] -contains "36")
  if($refIndexed -and $psStandard){
    P "$tag : SIDE EFFECT CONFIRMED - ref stays 256-indexed [$refCodes], psmux rewrote to standard [$psCodes]"
  } else {
    F "$tag : side effect NOT reproduced - ref [$refCodes] vs psmux [$psCodes]"
  }
}

# Basic color: rewrite is intended and correct (should match reference standard).
$g = $ps["BASICGRN"]
if($g -contains "32" -and -not (($g -contains "38") -and ($g -contains "5"))){
  P "BASICGRN : intended rewrite intact (32, bold-is-bright works)"
} else { F "BASICGRN : expected 32, got [$($g -join ',')]" }

# 256-indexed >15 must be untouched in BOTH.
$hiRef=$ref["IDX256HI"]; $hiPs=$ps["IDX256HI"]
if(($hiPs -contains "38") -and ($hiPs -contains "120")){
  P "IDX256HI : 256-indexed 120 passed through untouched in psmux [$($hiPs -join ',')]"
} else { F "IDX256HI : 256-indexed 120 was altered in psmux [$($hiPs -join ',')]" }

Write-Host "`n=== Results: Passed=$($script:Pass) Failed=$($script:Fail) ===" -ForegroundColor Cyan
exit $script:Fail
