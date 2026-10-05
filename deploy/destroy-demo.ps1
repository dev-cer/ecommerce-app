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
    [string]$NetworkLabel = "ecommerce-vpc-kcd-v2",
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

# Resource yang salah satu field-nya SAMA PERSIS dengan salah satu ID (bukan pencocokan nama/substring).
function Get-CivoById([string]$Kind, [string[]]$Ids) {
    $ids = @($Ids | Where-Object { $_ })
    if (-not $ids) { return @() }
    $raw = Invoke-Quiet { civo $Kind ls --region $Region -o json } | Out-String
    try { $items = @($raw | ConvertFrom-Json) } catch { return @() }
    return @($items | Where-Object {
        @($_.PSObject.Properties | Where-Object { $ids -contains "$($_.Value)" }).Count -gt 0
    })
}

# State Terraform bisa basi: mis. `terraform destroy` dihentikan (Ctrl+C) saat network "Still destroying",
# tetapi Civo tetap menyelesaikan penghapusannya. Provider lalu "memodifikasi" network yang sudah tidak ada
# ("error occurred while renaming the network" / civo_firewall "could not find network"). Network dianggap
# hilang bila setelah refresh label-nya kosong (provider tidak bisa membacanya) ATAU tidak ada di
# `civo network ls`. Semua resource di state lalu dikeluarkan (firewall, klaster, node pool bergantung pada
# network ini) agar Terraform membuat ulang dari nol.
function Repair-TerraformState([string]$Dir, [string]$Region = "jkt1") {
    Invoke-Quiet { terraform "-chdir=$Dir" apply -refresh-only -auto-approve -input=false } | Out-Null
    $state = Invoke-Quiet { terraform "-chdir=$Dir" show -json } | Out-String | ConvertFrom-Json -ErrorAction SilentlyContinue
    $resources = @($state.values.root_module.resources | Where-Object { $_.mode -eq "managed" })
    $network = $resources | Where-Object { $_.type -eq "civo_network" } | Select-Object -First 1
    if (-not $network) { return }
    $networkId = $network.values.id
    if ($network.values.region) { $Region = "$($network.values.region)".ToLower() }

    $unreadable = -not $network.values.label
    $listed = $null
    $raw = Invoke-Quiet { civo network ls --region $Region -o json } | Out-String
    try { $networks = @($raw | ConvertFrom-Json | Where-Object { $_.id }) } catch { $networks = @() }
    # Daftar kosong berarti civo CLI gagal (akun selalu punya network default): abaikan sumber ini.
    if ($networks.Count -gt 0) { $listed = @($networks | Where-Object { $_.id -eq $networkId }).Count -gt 0 }
    if (-not $unreadable -and $listed -ne $false) { return }

    Write-Info "network $networkId di Terraform state tidak bisa dipakai lagi (terbaca provider: $(if ($unreadable) { "tidak" } else { "ya" }), ada di civo: $(if ($null -eq $listed) { "tidak diketahui" } elseif ($listed) { "ya" } else { "tidak" })): membersihkan state basi..."
    if ($listed) {
        # Masih terdaftar tapi rusak/setengah terhapus: milik demo ini (ID dari state), coba hapus.
        Invoke-Quiet { civo network remove $networkId --region $Region -y } | Out-Null
    }
    foreach ($res in $resources) {
        Invoke-Quiet { terraform "-chdir=$Dir" state rm $res.address } | Out-Null
        Write-Info "  state rm $($res.address)"
    }
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
                # Hanya network buatan infra/main.tf (ID + label dari state) yang boleh dihapus.
                "civo_network" {
                    if (-not $NetworkId) { $NetworkId = $res.values.id }
                    if ($res.values.label) { $NetworkLabel = $res.values.label }
                }
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
    # Network TIDAK diambil dari klaster (bisa saja network default akun): hanya dari state atau -NetworkId.
    $network = $null
    if ($NetworkId) {
        $network = Get-CivoById "network" @($NetworkId) | Where-Object { $_.id -eq $NetworkId } | Select-Object -First 1
    }

    Write-Host ""
    Write-Host "  Klaster : $ClusterName ($(if ($ClusterId) { $ClusterId } else { '-' }))$(if (-not $cluster) { ' - sudah tidak ada' })"
    Write-Host "  Region  : $Region"
    Write-Host "  Network : $(if ($NetworkId) { "$NetworkLabel ($NetworkId)$(if (-not $network) { ' - sudah tidak ada' })" } else { '-' })"
    Write-Host "  Terraform state: $(if ($hasState) { $TerraformDir } else { 'tidak ada (lewati terraform destroy)' })"
    if (-not $ClusterId -and -not $NetworkId -and -not $hasState) {
        Write-Done "Tidak ada klaster, network, maupun Terraform state: tidak ada yang perlu dihapus."
        exit 0
    }
    if (-not $Force) {
        $answer = Read-Host "`nSemua resource di atas akan DIHAPUS PERMANEN. Ketik nama klaster untuk lanjut"
        if ($answer -ne $ClusterName) { throw "Dibatalkan." }
    }

    $ids = @{ ClusterName = $ClusterName; Region = $Region; NetworkLabel = $NetworkLabel }
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
        Repair-TerraformState $TerraformDir $Region
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
        $left += Get-CivoById $kind @($ClusterId, $NetworkId) | ForEach-Object { "$kind $($_.name) ($($_.id))" }
    }
    $left += Get-CivoById "firewall" @($NetworkId) | ForEach-Object { "firewall $($_.name) ($($_.id))" }
    $left += Get-CivoById "network" @($NetworkId) | Where-Object { $_.id -eq $NetworkId } |
        ForEach-Object { "network $($_.label) ($($_.id))" }
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
