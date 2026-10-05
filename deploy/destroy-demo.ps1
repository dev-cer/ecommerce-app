<#
.SYNOPSIS
    Membongkar demo KCD sampai bersih: aplikasi, LoadBalancer, volume, klaster, firewall, dan network Civo.

.DESCRIPTION
    Urutan penting: resource yang dibuat Kubernetes di akun Civo (LoadBalancer + IP publik dari Service,
    volume dari PVC) TIDAK dikelola Terraform. Kalau klaster langsung dihapus, resource itu tertinggal
    (tetap ditagih) dan network tidak bisa dihapus karena masih dipakai. Karena itu:

      1. Cluster Autoscaler & ArgoCD dihentikan (agar tidak membuat node/resource baru).
      2. Semua Service LoadBalancer & PVC dihapus dari dalam klaster, ditunggu sampai Civo melepasnya.
      3. terraform destroy: node pool, klaster, firewall, network.
      4. Sapu sisa di akun Civo yang terkait klaster/network ini (LB, volume, firewall, network).
      5. Konteks kubeconfig lokal dihapus.

    Hanya resource yang cocok dengan ID/nama klaster & network ini yang dihapus; resource lain di akun aman.

.EXAMPLE
    $env:CIVO_TOKEN = "<api key>"
    ./deploy/destroy-demo.ps1            # minta konfirmasi nama klaster
    ./deploy/destroy-demo.ps1 -Force     # tanpa konfirmasi
#>
[CmdletBinding()]
param(
    [string]$TerraformDir = (Join-Path $PSScriptRoot "../infra"),
    [string]$ClusterName = "ecommerce-kcd-cluster",
    [string]$Region = "jkt1",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$networkId = $null

function Write-Step([string]$Text) { Write-Host "`n$Text" -ForegroundColor Yellow }
function Write-Done([string]$Text) { Write-Host "✅ $Text" -ForegroundColor Green }
function Write-Info([string]$Text) { Write-Host "   $Text" -ForegroundColor DarkGray }
function Write-Warn([string]$Text) { Write-Host "⚠️  $Text" -ForegroundColor Yellow }

function Invoke-Native([string]$Description, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Description gagal (exit code $LASTEXITCODE)." }
}

# Perintah yang boleh gagal; mencegah NativeCommandError di Windows PowerShell 5.1.
function Invoke-Quiet([scriptblock]$Command) {
    $ErrorActionPreference = "Continue"
    & $Command 2>$null
}

function Wait-Until([string]$Description, [int]$TimeoutSeconds, [scriptblock]$Condition) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (& $Condition) { return $true }
        Write-Info "menunggu $Description..."
        Start-Sleep -Seconds 10
    }
    Write-Warn "Timeout menunggu $Description (akan disapu di langkah akhir)."
    return $false
}

# Daftar resource Civo (JSON) yang terkait klaster/network ini. Dicocokkan ke seluruh isi objek
# (cluster_id, network_id, nama) agar tidak bergantung pada nama field per versi civo CLI.
function Get-CivoLeftovers([string]$Kind, [string[]]$Needles) {
    $raw = Invoke-Quiet { civo $Kind ls --region $Region -o json } | Out-String
    if (-not $raw.Trim()) { return @() }
    try { $items = @($raw | ConvertFrom-Json) } catch { return @() }
    $needles = @($Needles | Where-Object { $_ })
    return @($items | Where-Object {
        $json = $_ | ConvertTo-Json -Compress -Depth 5
        @($needles | Where-Object { $json.Contains($_) }).Count -gt 0
    })
}

function Get-Label($Item) {
    foreach ($field in "name", "label", "hostname") { if ($Item.$field) { return $Item.$field } }
    return $Item.id
}

function Remove-CivoLeftovers([string]$Kind, [string[]]$Needles) {
    $found = Get-CivoLeftovers $Kind $Needles
    foreach ($item in $found) {
        Write-Info "menghapus $Kind sisa: $(Get-Label $item) ($($item.id))"
        Invoke-Quiet { civo $Kind remove $item.id --region $Region -y } | Out-Null
    }
    return $found.Count
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
    # Identitas klaster & network (sebelum dihapus, untuk sapu bersih di akhir)
    # ---------------------------------------------------------
    if ($hasState) {
        $tfName = Invoke-Quiet { terraform "-chdir=$TerraformDir" output -raw cluster_name }
        if ($LASTEXITCODE -eq 0 -and $tfName) { $ClusterName = $tfName }
        $tfRegion = Invoke-Quiet { terraform "-chdir=$TerraformDir" output -raw region }
        if ($LASTEXITCODE -eq 0 -and $tfRegion) { $Region = $tfRegion }
        $tfNetwork = Invoke-Quiet { terraform "-chdir=$TerraformDir" output -raw network_id }
        if ($LASTEXITCODE -eq 0 -and $tfNetwork -match '^[0-9a-f-]{36}$') { $networkId = $tfNetwork }
    }
    $cluster = $null
    $clusterJson = Invoke-Quiet { civo kubernetes show $ClusterName --region $Region -o json } | Out-String
    if ($LASTEXITCODE -eq 0) { $cluster = $clusterJson | ConvertFrom-Json -ErrorAction SilentlyContinue }
    $clusterId = $cluster.id
    if (-not $networkId -and $cluster) {
        # Nama field berbeda antar versi civo CLI (network_id / NetworkID): ambil properti yang mengandung "network".
        $networkId = $cluster.PSObject.Properties | Where-Object { $_.Name -match "network" -and "$($_.Value)" -match '^[0-9a-f-]{36}$' } |
            Select-Object -First 1 -ExpandProperty Value
    }
    $networkLabel = $null
    if ($networkId) {
        $networkLabel = (Get-CivoLeftovers "network" @($networkId) | Select-Object -First 1).label
    }

    Write-Host ""
    Write-Host "  Klaster : $ClusterName ($(if ($clusterId) { $clusterId } else { 'tidak ditemukan di Civo' }))"
    Write-Host "  Region  : $Region"
    Write-Host "  Network : $(if ($networkId) { "$networkLabel ($networkId)" } else { '-' })"
    Write-Host "  Terraform state: $(if ($hasState) { $TerraformDir } else { 'tidak ada (lewati terraform destroy)' })"
    if (-not $Force) {
        $answer = Read-Host "`nSemua resource di atas akan DIHAPUS PERMANEN. Ketik nama klaster untuk lanjut"
        if ($answer -ne $ClusterName) { throw "Dibatalkan." }
    }

    # ---------------------------------------------------------
    # 1-2. Bersihkan dari dalam klaster (LB & volume Civo dilepas dengan rapi)
    # ---------------------------------------------------------
    $reachable = $false
    if ($clusterId) {
        Write-Step "[1/5] Mengambil kubeconfig & menghentikan autoscaler/ArgoCD..."
        # Kubeconfig khusus di file sementara: perintah hapus di bawah TIDAK mungkin mengenai klaster lain
        # yang kebetulan sedang aktif di ~/.kube/config.
        $kubeconfig = Join-Path ([IO.Path]::GetTempPath()) "destroy-$ClusterName.kubeconfig"
        $config = Invoke-Quiet { civo kubernetes config $ClusterName --region $Region } | Out-String
        if ($LASTEXITCODE -eq 0 -and $config -match "apiVersion") {
            [IO.File]::WriteAllText($kubeconfig, $config, (New-Object Text.UTF8Encoding $false))
            $previousKubeconfig = $env:KUBECONFIG
            $env:KUBECONFIG = $kubeconfig
            Invoke-Quiet { kubectl cluster-info --request-timeout=15s } | Out-Null
            $reachable = $LASTEXITCODE -eq 0
        }
    }
    if ($reachable) {
        # Autoscaler bisa menambah node saat pod dihapus/dibuat ulang; ArgoCD (selfHeal) akan membuat ulang
        # Service/PVC yang kita hapus, jadi controller-nya dimatikan dulu.
        Invoke-Quiet { kubectl scale deployment --all -n cluster-autoscaler --replicas=0 } | Out-Null
        Invoke-Quiet { kubectl scale statefulset argocd-application-controller -n argocd --replicas=0 } | Out-Null
        Invoke-Quiet { kubectl delete job --all -n loadtest --wait=false } | Out-Null
        Write-Done "Autoscaler & ArgoCD dihentikan."

        Write-Step "[2/5] Menghapus Service LoadBalancer & volume (PVC) agar Civo melepas LB, IP publik & volume..."
        $lbs = Invoke-Quiet {
            kubectl get svc -A -o "jsonpath={range .items[?(@.spec.type=='LoadBalancer')]}{.metadata.namespace}/{.metadata.name}{'\n'}{end}"
        } | Where-Object { $_ }
        foreach ($lb in $lbs) {
            $ns, $name = $lb -split "/"
            Write-Info "Service $lb"
            Invoke-Quiet { kubectl delete svc $name -n $ns --wait=false } | Out-Null
        }

        # StatefulSet pemakai PVC dihapus dulu supaya volume tidak sedang ter-attach (kube-system tidak disentuh:
        # Civo CSI di sana yang menghapus volume).
        $pvcNamespaces = Invoke-Quiet {
            kubectl get pvc -A -o "jsonpath={range .items[*]}{.metadata.namespace}{'\n'}{end}"
        } | Where-Object { $_ -and $_ -ne "kube-system" } | Sort-Object -Unique
        foreach ($ns in $pvcNamespaces) {
            Write-Info "PVC di namespace $ns"
            Invoke-Quiet { kubectl delete statefulset --all -n $ns --wait=true --timeout=120s } | Out-Null
            Invoke-Quiet { kubectl delete pvc --all -n $ns --wait=false } | Out-Null
        }

        Wait-Until "LoadBalancer Civo dilepas" 300 {
            -not (Invoke-Quiet { kubectl get svc -A -o "jsonpath={.items[?(@.spec.type=='LoadBalancer')].metadata.name}" })
        } | Out-Null
        Wait-Until "volume Civo dihapus (PV)" 300 {
            -not (Invoke-Quiet { kubectl get pv -o name })
        } | Out-Null
        Write-Done "LoadBalancer & volume dari klaster sudah dilepas."
    } elseif ($clusterId) {
        Write-Warn "API klaster tidak bisa dihubungi (IP berubah / klaster rusak); LB & volume akan disapu lewat civo CLI."
    }

    # ---------------------------------------------------------
    # 3. Terraform destroy (node pool, klaster, firewall, network)
    # ---------------------------------------------------------
    if ($hasState) {
        Write-Step "[3/5] terraform destroy (klaster, node pool, firewall, network)..."
        Invoke-Native "terraform init" { terraform "-chdir=$TerraformDir" init -input=false }
        terraform "-chdir=$TerraformDir" destroy -auto-approve -input=false
        if ($LASTEXITCODE -ne 0) {
            # Biasanya network masih dipakai LB/volume yang tertinggal: sapu dulu lalu ulangi sekali.
            Write-Warn "terraform destroy gagal; menyapu resource yang masih menempel di network lalu mencoba lagi..."
            foreach ($kind in "loadbalancer", "volume") { Remove-CivoLeftovers $kind @($clusterId, $networkId, $ClusterName) | Out-Null }
            Start-Sleep -Seconds 30
            Invoke-Native "terraform destroy (ulang)" { terraform "-chdir=$TerraformDir" destroy -auto-approve -input=false }
        }
        Write-Done "Terraform destroy selesai."
    } elseif ($clusterId) {
        Write-Step "[3/5] Tidak ada Terraform state: menghapus klaster lewat civo CLI..."
        Invoke-Native "civo kubernetes remove" { civo kubernetes remove $ClusterName --region $Region -y }
    }

    # ---------------------------------------------------------
    # 4. Sapu sisa di akun Civo
    # ---------------------------------------------------------
    Write-Step "[4/5] Memeriksa sisa resource di akun Civo..."
    if ($clusterId) {
        Wait-Until "klaster terhapus dari Civo" 600 {
            Invoke-Quiet { civo kubernetes show $ClusterName --region $Region -o json } | Out-Null
            $LASTEXITCODE -ne 0
        } | Out-Null
    }
    $needles = @($clusterId, $networkId, $ClusterName)
    $removed = 0
    $removed += Remove-CivoLeftovers "loadbalancer" $needles
    $removed += Remove-CivoLeftovers "volume" $needles
    if ($networkId) {
        # Firewall & network hanya yang menempel di network klaster ini (bukan "default" milik akun).
        Start-Sleep -Seconds 10
        $removed += Remove-CivoLeftovers "firewall" @($networkId)
        $removed += Remove-CivoLeftovers "network" @($networkId)
    }
    if ($removed -gt 0) { Write-Info "$removed resource sisa dihapus." }

    # ---------------------------------------------------------
    # 5. Bersihkan kubeconfig lokal
    # ---------------------------------------------------------
    Write-Step "[5/5] Membersihkan kubeconfig lokal..."
    if ($kubeconfig) {
        $env:KUBECONFIG = $previousKubeconfig
        Remove-Item $kubeconfig -ErrorAction SilentlyContinue
    }
    $contexts = Invoke-Quiet { kubectl config get-contexts -o name } | Where-Object { $_ -like "*$ClusterName*" }
    foreach ($ctx in $contexts) {
        $kubeCluster = Invoke-Quiet { kubectl config view -o "jsonpath={.contexts[?(@.name=='$ctx')].context.cluster}" }
        $kubeUser = Invoke-Quiet { kubectl config view -o "jsonpath={.contexts[?(@.name=='$ctx')].context.user}" }
        Invoke-Quiet { kubectl config delete-context $ctx } | Out-Null
        if ($kubeCluster) { Invoke-Quiet { kubectl config delete-cluster $kubeCluster } | Out-Null }
        if ($kubeUser) { Invoke-Quiet { kubectl config unset "users.$kubeUser" } | Out-Null }
        Write-Info "konteks $ctx dihapus"
    }

    # ---------------------------------------------------------
    # Laporan akhir
    # ---------------------------------------------------------
    Write-Host ""
    $left = @()
    Invoke-Quiet { civo kubernetes show $ClusterName --region $Region -o json } | Out-Null
    if ($LASTEXITCODE -eq 0) { $left += "klaster $ClusterName" }
    foreach ($kind in "loadbalancer", "volume") {
        $left += Get-CivoLeftovers $kind $needles | ForEach-Object { "$kind $(Get-Label $_)" }
    }
    if ($networkId) {
        foreach ($kind in "firewall", "network") {
            $left += Get-CivoLeftovers $kind @($networkId) | ForEach-Object { "$kind $(Get-Label $_)" }
        }
    }
    if ($left) {
        Write-Warn "Masih tersisa (mungkin masih proses hapus, cek lagi 1-2 menit):"
        $left | ForEach-Object { Write-Host "     - $_" }
        Write-Info "Jalankan ulang skrip ini, atau hapus manual di dashboard Civo."
        exit 1
    }
    Write-Done "Bersih: klaster, node, LoadBalancer, IP publik, volume, firewall & network demo sudah terhapus."
    Write-Info "Image di ghcr.io tidak ikut dihapus (tidak ditagih Civo)."
}
catch {
    Write-Host "❌ ERROR: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
