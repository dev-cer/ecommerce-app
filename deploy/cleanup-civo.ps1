<#
.SYNOPSIS
    Pembersih resource Civo yang dibuat Kubernetes (LoadBalancer + IP publik, volume PVC), berdasarkan ID.

.DESCRIPTION
    LoadBalancer dan volume dibuat oleh klaster, bukan Terraform, sehingga tidak ikut terhapus oleh
    `terraform destroy` dan membuat network tersangkut ("Still destroying..."). Skrip ini dipanggil
    OTOMATIS oleh provisioner destroy di infra/main.tf, juga oleh deploy/destroy-demo.ps1:

      -Phase InCluster  sebelum klaster dihapus: matikan autoscaler/ArgoCD, hapus Service LoadBalancer
                        dan PVC dari dalam klaster, tunggu Civo melepasnya (fallback: sapu via civo CLI).
      -Phase Sweep      setelah klaster dihapus, sebelum network: hapus LB/volume yang masih menempel
                        di klaster/network ini, juga firewall lain di network (kecuali -KeepIds).
                        -IncludeNetwork ikut menghapus network-nya (sapu akhir).

    Hanya resource yang cocok dengan ID/nama klaster & network ini yang disentuh. Aman diulang.
#>
[CmdletBinding()]
param(
    [ValidateSet("InCluster", "Sweep")]
    [string]$Phase = "Sweep",
    [string]$ClusterName = "ecommerce-kcd-cluster",
    [string]$ClusterId,
    [string]$NetworkId,
    [string]$Region = "jkt1",
    [string[]]$KeepIds = @(),
    [switch]$IncludeNetwork,
    [int]$TimeoutSeconds = 300
)

$ErrorActionPreference = "Stop"

function Write-Info([string]$Text) { Write-Host "   [cleanup] $Text" -ForegroundColor DarkGray }
function Write-Warn([string]$Text) { Write-Host "   [cleanup] ⚠️  $Text" -ForegroundColor Yellow }

# Perintah yang boleh gagal; mencegah NativeCommandError di Windows PowerShell 5.1.
function Invoke-Quiet([scriptblock]$Command) {
    $ErrorActionPreference = "Continue"
    & $Command 2>$null
}

function Wait-Until([string]$Description, [int]$Seconds, [scriptblock]$Condition) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (& $Condition) { return $true }
        Write-Info "menunggu $Description..."
        Start-Sleep -Seconds 10
    }
    Write-Warn "timeout menunggu $Description."
    return $false
}

# Resource Civo (JSON) yang terkait klaster/network ini. Dicocokkan ke seluruh isi objek (cluster_id,
# network_id, nama) agar tidak bergantung pada nama field per versi civo CLI.
function Get-CivoResources([string]$Kind, [string[]]$Needles) {
    $needles = @($Needles | Where-Object { $_ })
    if (-not $needles) { return @() }
    $raw = Invoke-Quiet { civo $Kind ls --region $Region -o json } | Out-String
    if (-not $raw.Trim()) { return @() }
    try { $items = @($raw | ConvertFrom-Json) } catch { return @() }
    return @($items | Where-Object {
        $item = $_
        $json = $item | ConvertTo-Json -Compress -Depth 5
        ($KeepIds -notcontains $item.id) -and (@($needles | Where-Object { $json.Contains($_) }).Count -gt 0)
    })
}

function Get-Label($Item) {
    foreach ($field in "name", "label", "hostname") { if ($Item.$field) { return $Item.$field } }
    return $Item.id
}

function Remove-CivoResources([string]$Kind, [string[]]$Needles) {
    $found = Get-CivoResources $Kind $Needles
    foreach ($item in $found) {
        Write-Info "hapus $Kind $(Get-Label $item) ($($item.id))"
        Invoke-Quiet { civo $Kind remove $item.id --region $Region -y } | Out-Null
    }
    return $found.Count
}

# LB & volume milik klaster/network ini dihapus lalu ditunggu sampai benar-benar hilang dari akun.
function Invoke-Sweep {
    $needles = @($ClusterId, $NetworkId, $ClusterName)
    $removed = 0
    foreach ($kind in "loadbalancer", "volume") { $removed += Remove-CivoResources $kind $needles }
    if ($removed -gt 0) {
        Wait-Until "Civo melepas LB/volume" $TimeoutSeconds {
            (@(Get-CivoResources "loadbalancer" $needles) + @(Get-CivoResources "volume" $needles)).Count -eq 0
        } | Out-Null
    }
    if ($NetworkId) {
        # Firewall lain di network ini (bukan milik Terraform, lihat -KeepIds) juga menahan penghapusan network.
        Remove-CivoResources "firewall" @($NetworkId) | Out-Null
        if ($IncludeNetwork) { Remove-CivoResources "network" @($NetworkId) | Out-Null }
    }
}

if ($Phase -eq "InCluster") {
    # Kubeconfig khusus di file sementara: perintah hapus tidak mungkin mengenai klaster lain di ~/.kube/config.
    $kubeconfig = Join-Path ([IO.Path]::GetTempPath()) "cleanup-$ClusterName.kubeconfig"
    $config = Invoke-Quiet { civo kubernetes config $ClusterName --region $Region } | Out-String
    $reachable = $false
    if ($LASTEXITCODE -eq 0 -and $config -match "apiVersion") {
        [IO.File]::WriteAllText($kubeconfig, $config, (New-Object Text.UTF8Encoding $false))
        $previousKubeconfig = $env:KUBECONFIG
        $env:KUBECONFIG = $kubeconfig
        Invoke-Quiet { kubectl cluster-info --request-timeout=15s } | Out-Null
        $reachable = $LASTEXITCODE -eq 0
    }

    if ($reachable) {
        Write-Info "klaster ${ClusterName}: matikan autoscaler & ArgoCD (agar tidak membuat ulang resource)"
        Invoke-Quiet { kubectl scale deployment --all -n cluster-autoscaler --replicas=0 } | Out-Null
        Invoke-Quiet { kubectl scale statefulset argocd-application-controller -n argocd --replicas=0 } | Out-Null
        Invoke-Quiet { kubectl delete job --all -n loadtest --wait=false } | Out-Null

        $lbs = Invoke-Quiet {
            kubectl get svc -A -o "jsonpath={range .items[?(@.spec.type=='LoadBalancer')]}{.metadata.namespace}/{.metadata.name}{'\n'}{end}"
        } | Where-Object { $_ }
        foreach ($lb in $lbs) {
            $ns, $name = $lb -split "/"
            Write-Info "hapus Service LoadBalancer $lb"
            Invoke-Quiet { kubectl delete svc $name -n $ns --wait=false } | Out-Null
        }
        # StatefulSet pemakai PVC dihapus dulu agar volume ter-detach (kube-system tidak disentuh: Civo CSI
        # di sana yang menghapus volume).
        $pvcNamespaces = Invoke-Quiet {
            kubectl get pvc -A -o "jsonpath={range .items[*]}{.metadata.namespace}{'\n'}{end}"
        } | Where-Object { $_ -and $_ -ne "kube-system" } | Sort-Object -Unique
        foreach ($ns in $pvcNamespaces) {
            Write-Info "hapus PVC di namespace $ns"
            Invoke-Quiet { kubectl delete statefulset --all -n $ns --wait=true --timeout=120s } | Out-Null
            Invoke-Quiet { kubectl delete pvc --all -n $ns --wait=false } | Out-Null
        }
        Wait-Until "LoadBalancer dilepas" $TimeoutSeconds {
            -not (Invoke-Quiet { kubectl get svc -A -o "jsonpath={.items[?(@.spec.type=='LoadBalancer')].metadata.name}" })
        } | Out-Null
        Wait-Until "volume dihapus" $TimeoutSeconds { -not (Invoke-Quiet { kubectl get pv -o name }) } | Out-Null
    } else {
        Write-Warn "API klaster $ClusterName tidak bisa dihubungi; sapu LB/volume lewat civo CLI."
    }
    if ($kubeconfig) {
        $env:KUBECONFIG = $previousKubeconfig
        Remove-Item $kubeconfig -ErrorAction SilentlyContinue
    }
    # Sisa yang tidak terhapus dari dalam klaster (mis. API tidak terjangkau).
    foreach ($kind in "loadbalancer", "volume") { Remove-CivoResources $kind @($ClusterId, $ClusterName) | Out-Null }
} else {
    Invoke-Sweep
}
Write-Info "selesai ($Phase)."
exit 0
