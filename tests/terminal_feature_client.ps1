# Launch a real client in this helper's own classic console or ConPTY.
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Namespace,
    [Parameter(Mandatory)][string]$Config,
    [Parameter(Mandatory)][string]$InfoPath
)
$ErrorActionPreference = 'Stop'
if ($env:PSMUX_DISPOSABLE_GUEST -ne '1' -and $env:PSMUX_TEST_SANDBOX -ne '1' -and $env:GITHUB_ACTIONS -ne 'true') {
    throw 'Run in a disposable Windows guest, test sandbox, or GitHub Actions worker.'
}
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class FeatureClientConsole {
    [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr window, StringBuilder name, int count);
}
'@
$window = [FeatureClientConsole]::GetConsoleWindow()
$class = [Text.StringBuilder]::new(64)
if ([FeatureClientConsole]::GetClassName($window, $class, $class.Capacity) -le 0) { throw 'Console class query failed.' }
$start = [Diagnostics.ProcessStartInfo]::new($Exe)
$start.UseShellExecute = $false
foreach ($word in @('-L', $Namespace, '-f', $Config, 'attach', '-t', 'work')) { $start.ArgumentList.Add($word) }
$client = [Diagnostics.Process]::Start($start)
try {
    @{Pid=$client.Id; Class=$class.ToString()} |
        ConvertTo-Json | Set-Content "$InfoPath.tmp"
    Move-Item "$InfoPath.tmp" $InfoPath
    $client.WaitForExit()
    exit $client.ExitCode
} finally {
    if (-not $client.HasExited) {
        $client.Kill()
        if (-not $client.WaitForExit(15000)) { throw 'Owned frame did not exit after termination.' }
    }
}
