# Stop a process a suite started, and the processes it started in turn.
#
# taskkill /T picks children by ParentProcessId alone, so a long lived process
# whose parent has exited (amesh's daemon, for one) is swept in as soon as that
# dead parent's PID is reused by the terminal a suite launches. This walk only
# follows a child created after its parent, which a reused PID can never
# satisfy, and never stops amesh.exe, which a suite never starts.
function Stop-OwnTree {
    param([Parameter(Mandatory)][int]$RootPid)
    $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, CreationDate -EA SilentlyContinue)
    $root = $all | Where-Object { [int]$_.ProcessId -eq $RootPid } | Select-Object -First 1
    if (-not $root) { return }
    $order = New-Object System.Collections.Generic.List[object]
    $queue = New-Object System.Collections.Generic.Queue[object]
    $queue.Enqueue($root)
    while ($queue.Count -gt 0) {
        $p = $queue.Dequeue()
        $order.Add($p)
        foreach ($c in $all) {
            if ([int]$c.ParentProcessId -ne [int]$p.ProcessId -or [int]$c.ProcessId -eq [int]$p.ProcessId) { continue }
            if (-not $c.CreationDate -or -not $p.CreationDate -or $c.CreationDate -lt $p.CreationDate) { continue }
            if ($c.Name -ieq 'amesh.exe') { continue }
            $queue.Enqueue($c)
        }
    }
    # Children first, so none is re-parented and missed while its parent dies.
    for ($i = $order.Count - 1; $i -ge 0; $i--) {
        try { Stop-Process -Id ([int]$order[$i].ProcessId) -Force -EA Stop } catch { }
    }
}
