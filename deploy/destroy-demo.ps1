<#
.SYNOPSIS
    Membongkar demo KCD sampai bersih: aplikasi, LoadBalancer, volume, klaster, firewall, dan network Civo.

.DESCRIPTION
    Tanpa langkah manual. ID klaster, network & firewall dibaca otomatis dari Terraform state (tetap ada
    walau klaster sudah terhapus), lalu:

      1. Klaster masih ada  -> cleanup-civo.ps1 -Phase InCluster (hapus Service LoadBalancer & PVC dari dalam).
         Klaster sudah hilang -> cleanup-civo.ps1 -Phase Sweep (hapus LB/volume sisa berdasarkan ID).
      2. terraform destroy. Provisioner destroy di infra/main.tf juga menjalankan pembersihan yang sama,
         jadi `terraform destroy` langsung pun tetap bersih. Bila gagal: sapu lagi lalu ulangi sekali.
      3. Sapu akhir berdasarkan ID (LB, volume, firewall, network) + hapus konteks kubeconfig lokal.
      4. Laporan: exit 1 bila masih ada yang tersisa (aman dijalankan ulang).

.EXAMPLE
    $env:CIVO_TOKEN = "<api key>"
    ./deploy/destroy-demo.ps1            # minta ketik nama klaster sebagai konfirmasi
    ./deploy/destroy-demo.ps1 -Force     # tanpa konfirmasi
#>
[CmdletBinding()]
param(
    [string]$TerraformDir = (Join-Path $PSScriptRoot "../infra"),
    [string]$ClusterName = "ecommerce-kcd-cluster",
    [string]$Region = "jkt1",
    # Opsional; normalnya terbaca otomatis dari Terraform state / civo CLI.
    [string]$ClusterId,
    [string]$NetworkId,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$Cleanup = Join-Path $PSScriptRoot "cleanup-civo.ps1"

function Write-Step([string]$Text) { Write-Host "`n$Text" -ForegroundColor Yellow }
function Write-Done([string]$Text) { Write-Host "✅ $Text" -ForegroundColor Green }
function Write-Info([string]$Text) { Write-Host "   $Text" -ForegroundColor DarkGray }
function Write-Warn([string]$Text) { Write-Host "⚠️  $Text" -ForegroundColor Yellow }

# Perintah yang boleh gagal; mencegah NativeCommandError di Windows PowerShell 5.1.
function Invoke-Quiet([scriptblock]$Command) {
    $ErrorActionPreference = "Continue"
    & $Command 2>$null
}

function Get-CivoById([string]$Kind, [string[]]$Needles) {
    $needles = @($Needles | Where-Object { $_ })
    if (-not $needles) { return @() }
    $raw = Invoke-Quiet { civo $Kind ls --region $Region -o json } | Out-String
    try { $items = @($raw | ConvertFrom-Json) } catch { return @() }
    return @($items | Where-Object {
        $json = $_ | ConvertTo-Json -Compress -Depth 5
        @($needles | Where-Object { $json.Contains($_) }).Count -gt 0
    })
}

function Test-ClusterExists {
    Invoke-Quiet { civo kubernetes show $ClusterName --region $Region -o json } | Out-Null
    return $LASTEXITCODE -eq 0
}

try {
    Write-Host "=====================================================" -ForegroundColor Cyan
    Write-Host " KCD BANDUNG 2026: BONGKAR DEMO CIVO SAMPAI BERSIH" -ForegroundColor Cyan
    Write-Host "=====================================================" -ForegroundColor Cyan

    $missing = "terraform", "civo", "kubectl" | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
    if ($missing) { throw "Tool belum terpasang: $($missing -join ', ')" }
    if (-not $env:CIVO_TOKEN) { Write-Warn "`$env:CIVO_TOKEN kosong: provider Terraform Civo butuh token ini." }
    $hasState = Test-Path (Join-Path $TerraformDir "terraform.tfstate")

    # ---------------------------------------------------------
    # ID otomatis dari Terraform state & civo CLI
    # ---------------------------------------------------------
    $firewallIds = @()
    if ($hasState) {
        $tfState = Invoke-Quiet { terraform "-chdir=$TerraformDir" show -json } | Out-String |
            ConvertFrom-Json -ErrorAction SilentlyContinue
        foreach ($res in @($tfState.values.root_module.resources)) {
            switch ($res.type) {
                "civo_network" { if (-not $NetworkId) { $NetworkId = $res.values.id } }
                "civo_firewall" { $firewallIds += $res.values.id }
                "civo_kubernetes_cluster" {
                    if (-not $ClusterId) { $ClusterId = $res.values.id }
                    $ClusterName = $res.values.name
                    if ($res.values.region) { $Region = $res.values.region }
                }
            }
        }
    }
    $cluster = $null
    $clusterJson = Invoke-Quiet { civo kubernetes show $ClusterName --region $Region -o json } | Out-String
    if ($LASTEXITCODE -eq 0) { $cluster = $clusterJson | ConvertFrom-Json -ErrorAction SilentlyContinue }
    if ($cluster.id) { $ClusterId = $cluster.id }
    if (-not $NetworkId -and $cluster) {
        # Nama field berbeda antar versi civo CLI (network_id / NetworkID): ambil properti yang mengandung "network".
        $NetworkId = $cluster.PSObject.Properties |
            Where-Object { $_.Name -match "network" -and "$($_.Value)" -match '^[0-9a-f-]{36}$' } |
            Select-Object -First 1 -ExpandProperty Value
    }
    $networkLabel = (Get-CivoById "network" @($NetworkId) | Select-Object -First 1).label

    Write-Host ""
    Write-Host "  Klaster : $ClusterName ($(if ($ClusterId) { $ClusterId } else { '-' }))$(if (-not $cluster) { ' - sudah tidak ada' })"
    Write-Host "  Region  : $Region"
    Write-Host "  Network : $(if ($NetworkId) { "$networkLabel ($NetworkId)" } else { '-' })"
    Write-Host "  Terraform state: $(if ($hasState) { $TerraformDir } else { 'tidak ada (lewati terraform destroy)' })"
    if (-not $ClusterId -and -not $NetworkId -and -not $hasState) {
        Write-Done "Tidak ada klaster, network, maupun Terraform state: tidak ada yang perlu dihapus."
        exit 0
    }
    if (-not $Force) {
        $answer = Read-Host "`nSemua resource di atas akan DIHAPUS PERMANEN. Ketik nama klaster untuk lanjut"
        if ($answer -ne $ClusterName) { throw "Dibatalkan." }
    }

    $ids = @{ ClusterName = $ClusterName; Region = $Region }
    if ($ClusterId) { $ids.ClusterId = $ClusterId }
    if ($NetworkId) { $ids.NetworkId = $NetworkId }

    # ---------------------------------------------------------
    # 1. LoadBalancer & volume (dibuat Kubernetes, bukan Terraform)
    # ---------------------------------------------------------
    Write-Step "[1/4] Melepas LoadBalancer, IP publik & volume milik klaster..."
    if ($cluster) {
        & $Cleanup -Phase InCluster @ids
    } else {
        # Firewall milik Terraform dibiarkan: Terraform sendiri yang menghapusnya.
        & $Cleanup -Phase Sweep -KeepIds $firewallIds @ids
    }

    # ---------------------------------------------------------
    # 2. Terraform destroy (node pool, klaster, firewall, network)
    # ---------------------------------------------------------
    if ($hasState) {
        Write-Step "[2/4] terraform destroy (klaster, node pool, firewall, network)..."
        terraform "-chdir=$TerraformDir" init -input=false | Out-Null
        terraform "-chdir=$TerraformDir" destroy -auto-approve -input=false
        if ($LASTEXITCODE -ne 0) {
            Write-Warn "terraform destroy gagal; menyapu sisa berdasarkan ID lalu mencoba sekali lagi..."
            & $Cleanup -Phase Sweep -KeepIds $firewallIds @ids
            terraform "-chdir=$TerraformDir" destroy -auto-approve -input=false
            if ($LASTEXITCODE -ne 0) { throw "terraform destroy tetap gagal; jalankan ulang skrip ini." }
        }
        Write-Done "Terraform destroy selesai."
    } elseif (Test-ClusterExists) {
        Write-Step "[2/4] Tidak ada Terraform state: menghapus klaster lewat civo CLI..."
        Invoke-Quiet { civo kubernetes remove $ClusterName --region $Region -y } | Out-Null
    }

    # ---------------------------------------------------------
    # 3. Sapu akhir berdasarkan ID + kubeconfig lokal
    # ---------------------------------------------------------
    Write-Step "[3/4] Sapu akhir berdasarkan ID & bersihkan kubeconfig lokal..."
    $deadline = (Get-Date).AddMinutes(10)
    while ((Test-ClusterExists) -and (Get-Date) -lt $deadline) {
        Write-Info "menunggu klaster terhapus dari Civo..."
        Start-Sleep -Seconds 10
    }
    & $Cleanup -Phase Sweep -IncludeNetwork @ids

    $contexts = Invoke-Quiet { kubectl config get-contexts -o name } | Where-Object { $_ -like "*$ClusterName*" }
    foreach ($ctx in $contexts) {
        $kubeCluster = Invoke-Quiet { kubectl config view -o "jsonpath={.contexts[?(@.name=='$ctx')].context.cluster}" }
        $kubeUser = Invoke-Quiet { kubectl config view -o "jsonpath={.contexts[?(@.name=='$ctx')].context.user}" }
        Invoke-Quiet { kubectl config delete-context $ctx } | Out-Null
        if ($kubeCluster) { Invoke-Quiet { kubectl config delete-cluster $kubeCluster } | Out-Null }
        if ($kubeUser) { Invoke-Quiet { kubectl config unset "users.$kubeUser" } | Out-Null }
        Write-Info "konteks kubeconfig $ctx dihapus"
    }

    # ---------------------------------------------------------
    # 4. Laporan
    # ---------------------------------------------------------
    Write-Step "[4/4] Memeriksa akun Civo..."
    $left = @()
    if (Test-ClusterExists) { $left += "klaster $ClusterName" }
    foreach ($kind in "loadbalancer", "volume") {
        $left += Get-CivoById $kind @($ClusterId, $NetworkId, $ClusterName) | ForEach-Object { "$kind $($_.name) ($($_.id))" }
    }
    foreach ($kind in "firewall", "network") {
        $left += Get-CivoById $kind @($NetworkId) | ForEach-Object { "$kind $(if ($_.name) { $_.name } else { $_.label }) ($($_.id))" }
    }
    if ($left) {
        Write-Warn "Masih tersisa (biasanya Civo masih memproses; jalankan ulang skrip ini 1-2 menit lagi):"
        $left | ForEach-Object { Write-Host "     - $_" }
        exit 1
    }
    Write-Done "Bersih: klaster, node, LoadBalancer, IP publik, volume, firewall & network demo sudah terhapus."
    Write-Info "Image di ghcr.io tidak ikut dihapus (tidak ditagih Civo)."
}
catch {
    Write-Host "❌ ERROR: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
