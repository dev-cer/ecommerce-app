<#
.SYNOPSIS
    Pantau rantai autoscaling secara live: beban CPU -> HPA -> pod Pending -> Cluster Autoscaler -> node baru.

.DESCRIPTION
    Jalankan di terminal kedua selama ./stress-test.ps1 -Profile scale. Setiap beberapa detik menampilkan
    CPU & replika HPA, jumlah pod per status, node app-pool, dan event terakhir Cluster Autoscaler.

.EXAMPLE
    ./watch-scale.ps1              # refresh tiap 5 detik, Ctrl+C untuk berhenti
#>
[CmdletBinding()]
param(
    [int]$IntervalSeconds = 5,
    [string]$Pool = "app-pool"
)

function Invoke-Quiet([scriptblock]$Command) {
    $ErrorActionPreference = "Continue"
    & $Command 2>$null
}

while ($true) {
    $hpa = Invoke-Quiet { kubectl get hpa ecommerce-rust -n default -o json } | Out-String | ConvertFrom-Json
    $pods = (Invoke-Quiet { kubectl get pods -n default -l app=ecommerce-rust -o json } | Out-String | ConvertFrom-Json).items
    $nodes = (Invoke-Quiet { kubectl get nodes -l "kubernetes.civo.com/civo-node-pool=$Pool" -o json } | Out-String | ConvertFrom-Json).items
    $events = Invoke-Quiet {
        kubectl get events -n default --field-selector reason=TriggeredScaleUp --sort-by=.lastTimestamp -o "jsonpath={range .items[*]}{.lastTimestamp} {.message}{'\n'}{end}"
    }

    $cpu = ($hpa.status.currentMetrics | Where-Object { $_.resource.name -eq "cpu" }).resource.current.averageUtilization
    $target = $hpa.spec.metrics[0].resource.target.averageUtilization
    $running = @($pods | Where-Object { $_.status.phase -eq "Running" }).Count
    $pending = @($pods | Where-Object { $_.status.phase -eq "Pending" }).Count
    $ready = @($nodes | Where-Object { ($_.status.conditions | Where-Object type -eq "Ready").status -eq "True" }).Count

    Clear-Host
    Write-Host "NusaMart · autoscaling live   $(Get-Date -Format 'HH:mm:ss')   (Ctrl+C untuk berhenti)" -ForegroundColor Cyan
    Write-Host ""
    Write-Host ("  1. CPU vs target HPA : {0}% / {1}%" -f $(if ($null -ne $cpu) { $cpu } else { "?" }), $target)
    Write-Host ("  2. Replika HPA       : {0} (diinginkan {1}, maks {2})" -f $hpa.status.currentReplicas, $hpa.status.desiredReplicas, $hpa.spec.maxReplicas)
    Write-Host ("  3. Pod aplikasi      : {0} Running, " -f $running) -NoNewline
    Write-Host ("{0} Pending" -f $pending) -ForegroundColor $(if ($pending -gt 0) { "Yellow" } else { "Gray" })
    Write-Host ("  4. {0,-17}: {1} Ready dari {2}" -f "Node $Pool", $ready, $nodes.Count) -ForegroundColor $(if ($nodes.Count -gt 2) { "Green" } else { "Gray" })
    Write-Host ""
    Write-Host "  Event Cluster Autoscaler (TriggeredScaleUp):" -ForegroundColor DarkGray
    $last = @($events | Where-Object { $_ }) | Select-Object -Last 3
    if ($last) { $last | ForEach-Object { Write-Host "    $_" } } else { Write-Host "    (belum ada - butuh pod Pending dulu)" -ForegroundColor DarkGray }

    Start-Sleep -Seconds $IntervalSeconds
}
