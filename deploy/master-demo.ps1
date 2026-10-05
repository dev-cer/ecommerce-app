<#
.SYNOPSIS
    KCD Bandung 2026 - End-to-end demo Civo Kubernetes: Terraform -> ArgoCD -> Jenkins -> Prometheus/Grafana.

.DESCRIPTION
    Alur demo:
      ACT -1  Terraform membangun klaster Civo.
      ACT 0   ArgoCD dipasang di system-pool.
      ACT 1   ArgoCD memantau folder k8s/ di repo ecommerce-app (GitOps).
      ACT 2   Jenkins dipasang + job pipeline otomatis (lint, scan, build Kaniko, Trivy, update manifest).
      ACT 3   Prometheus + Grafana, lengkap dengan dashboard NusaMart.

    Kredensial TIDAK disimpan di skrip. Set dulu environment variable:
      $env:GITHUB_USER  = "dev-cer"
      $env:GITHUB_TOKEN = "<PAT dengan scope: repo, write:packages, read:packages>"

.EXAMPLE
    ./deploy/master-demo.ps1                          # memakai Terraform di folder infra/
    ./deploy/master-demo.ps1 -TerraformDir D:\lain   # folder Terraform lain
#>
[CmdletBinding()]
param(
    [string]$TerraformDir = (Join-Path $PSScriptRoot "../infra"),
    [string]$AppRepo      = "https://github.com/dev-cer/ecommerce-app.git",
    [string]$AppBranch    = "main",
    [string]$Registry     = "ghcr.io",
    [string]$GitHubUser   = $env:GITHUB_USER,
    [string]$GitHubToken  = $env:GITHUB_TOKEN
)

$ErrorActionPreference = "Stop"
$SystemPool = @{ "kubernetes.civo.com/civo-node-pool" = "system-pool" }

# ---------------------------------------------------------
# Helper
# ---------------------------------------------------------
function Write-Act([string]$Text)  { Write-Host "`n$Text" -ForegroundColor Yellow }
function Write-Done([string]$Text) { Write-Host "✅ $Text" -ForegroundColor Green }
function Write-Info([string]$Text) { Write-Host "   $Text" -ForegroundColor DarkGray }

# Perintah native (kubectl/helm/terraform) tidak melempar exception saat gagal,
# jadi exit code-nya dicek manual di sini.
function Invoke-Native([string]$Description, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Description gagal (exit code $LASTEXITCODE)." }
}

# Perintah native yang boleh gagal (cek status/ambil nilai). ErrorActionPreference lokal "Continue"
# mencegah Windows PowerShell 5.1 melempar NativeCommandError saat kubectl menulis ke stderr.
function Invoke-Quiet([scriptblock]$Command) {
    $ErrorActionPreference = "Continue"
    & $Command 2>$null
}

# Menulis file sementara tanpa BOM (Out-File di PowerShell 5.1 menambah BOM yang bisa merusak YAML/JSON).
function New-TempFile([string]$Content, [string]$Extension = ".yaml") {
    $path = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName() + $Extension)
    [IO.File]::WriteAllText($path, $Content, (New-Object Text.UTF8Encoding $false))
    return $path
}

function Invoke-KubectlApply([string]$Description, [string]$Yaml) {
    $file = New-TempFile $Yaml
    try { Invoke-Native $Description { kubectl apply -f $file } } finally { Remove-Item $file -ErrorAction SilentlyContinue }
}

# --patch-file menghindari masalah quoting JSON di PowerShell 5.1.
function Invoke-KubectlPatch([string]$Namespace, [string]$Target, [string]$Type, [object]$Patch) {
    $file = New-TempFile ($Patch | ConvertTo-Json -Depth 10 -Compress) ".json"
    try { Invoke-Native "Patch $Target" { kubectl patch $Target -n $Namespace --type $Type --patch-file $file } }
    finally { Remove-Item $file -ErrorAction SilentlyContinue }
}

function New-Namespace([string]$Name) {
    Invoke-Native "Membuat namespace $Name" {
        kubectl create namespace $Name --dry-run=client -o yaml | kubectl apply -f -
    }
}

function Set-RegistrySecret([string]$Namespace) {
    Invoke-Native "Membuat secret ghcr-creds di $Namespace" {
        kubectl create secret docker-registry ghcr-creds -n $Namespace `
            --docker-server=$Registry --docker-username=$GitHubUser --docker-password=$GitHubToken `
            --dry-run=client -o yaml | kubectl apply -f -
    }
}

function Install-HelmChart([string]$Release, [string]$Chart, [string]$Namespace, [string]$Values) {
    # Run sebelumnya terhenti (Ctrl+C/timeout) → release tertinggal "pending-*" dan Helm menolak
    # operasi baru. Bersihkan dulu supaya skrip aman dijalankan ulang.
    $status = (Invoke-Quiet { helm status $Release -n $Namespace -o json } | Out-String | ConvertFrom-Json -ErrorAction SilentlyContinue).info.status
    if ($status -eq "pending-install") {
        Write-Info "release $Release tertinggal di status $status, menghapus lalu install ulang..."
        Invoke-Native "Helm uninstall $Release" { helm uninstall $Release -n $Namespace --wait }
    } elseif ($status -like "pending-*") {
        Write-Info "release $Release tertinggal di status $status, rollback dulu..."
        Invoke-Native "Helm rollback $Release" { helm rollback $Release -n $Namespace --wait }
    }

    $file = New-TempFile $Values
    try {
        Invoke-Native "Helm install $Release" {
            helm upgrade --install $Release $Chart -n $Namespace -f $file --timeout 15m
        }
    } finally { Remove-Item $file -ErrorAction SilentlyContinue }
    Wait-Workloads $Namespace
}

# Tunggu semua Deployment/StatefulSet di namespace siap, sambil menampilkan progres
# (helm --wait diam saja selama belasan menit, membingungkan saat live demo).
function Wait-Workloads([string]$Namespace, [int]$TimeoutMinutes = 15) {
    $workloads = Invoke-Quiet { kubectl get "deployment,statefulset" -n $Namespace -o name }
    foreach ($workload in $workloads) {
        Write-Info "menunggu $workload ..."
        kubectl rollout status $workload -n $Namespace --timeout="$($TimeoutMinutes)m"
        if ($LASTEXITCODE -ne 0) {
            Show-Diagnostics $Namespace
            throw "$workload di namespace $Namespace tidak kunjung siap."
        }
    }
}

# Ringkasan untuk debugging: pod yang belum Running + event terbaru di namespace tersebut.
function Show-Diagnostics([string]$Namespace) {
    Write-Host "`n--- Pod yang belum siap ($Namespace) ---" -ForegroundColor DarkYellow
    Invoke-Quiet { kubectl get pods -n $Namespace -o wide } | Where-Object { $_ -notmatch '\s(\d+)/\1\s+Running' } | Out-Host
    Write-Host "--- Event terakhir ($Namespace) ---" -ForegroundColor DarkYellow
    Invoke-Quiet { kubectl get events -n $Namespace --sort-by=.lastTimestamp } | Select-Object -Last 12 | Out-Host
}

# Civo CSI membuat volume (PVC Postgres). Pernah teramati unduhan image-nya tersangkut di satu node;
# jika belum siap dalam 3 menit, pod di-restart sekali (StatefulSet membuatnya ulang otomatis).
function Confirm-CsiReady {
    $sts = "statefulset/civo-csi-controller"
    if (-not (Invoke-Quiet { kubectl get $sts -n kube-system -o name })) { return }
    Write-Info "memeriksa Civo CSI (pembuat volume)..."
    kubectl rollout status $sts -n kube-system --timeout=180s | Out-Null
    if ($LASTEXITCODE -eq 0) { return }
    Write-Info "CSI belum siap, me-restart civo-csi-controller-0 sekali..."
    Invoke-Quiet { kubectl delete pod civo-csi-controller-0 -n kube-system --wait=false } | Out-Null
    kubectl rollout status $sts -n kube-system --timeout=300s | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "⚠️  Civo CSI belum siap: volume Postgres akan tertunda. Cek: kubectl describe pod civo-csi-controller-0 -n kube-system" -ForegroundColor Yellow
    }
}

function Wait-Until([string]$Description, [int]$TimeoutSeconds, [scriptblock]$Condition) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (& $Condition) { return }
        Write-Info "menunggu $Description..."
        Start-Sleep -Seconds 10
    }
    throw "Timeout menunggu $Description."
}

# Password alfanumerik (aman dipakai di URL koneksi tanpa escaping).
function New-RandomPassword([int]$Length = 32) {
    $chars = [char[]]"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789"
    $bytes = New-Object byte[] $Length
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    return -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
}

function Get-SecretValue([string]$Namespace, [string]$Name, [string]$Key) {
    $encoded = Invoke-Quiet { kubectl get secret $Name -n $Namespace -o "jsonpath={.data.$Key}" }
    if (-not $encoded) { return "<belum tersedia>" }
    return [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($encoded))
}

function Get-ServiceAddress([string]$Namespace, [string]$Name, [string]$Scheme = "http", [string]$Port = "") {
    $ingress = Invoke-Quiet { kubectl get svc $Name -n $Namespace -o "jsonpath={.status.loadBalancer.ingress[0]['ip','hostname']}" }
    if (-not $ingress) { return "<pending> (cek: kubectl get svc $Name -n $Namespace)" }
    return "${Scheme}://$ingress$(if ($Port) { ":$Port" })"
}

# ---------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------
Write-Host "=====================================================" -ForegroundColor Cyan
Write-Host " KCD BANDUNG 2026: END-TO-END CIVO KUBERNETES DEMO" -ForegroundColor Cyan
Write-Host "=====================================================" -ForegroundColor Cyan

try {
    $missing = "terraform", "civo", "kubectl", "helm" | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
    if ($missing) { throw "Tool belum terpasang: $($missing -join ', ')" }
    if (-not $GitHubUser -or -not $GitHubToken) {
        throw 'Set $env:GITHUB_USER dan $env:GITHUB_TOKEN terlebih dahulu (jangan tulis token di skrip).'
    }

    # ---------------------------------------------------------
    # ACT -1: Infrastructure as Code (Terraform)
    # ---------------------------------------------------------
    Write-Act "[ACT -1] Membangun klaster & network Civo via Terraform..."
    if (-not (Test-Path (Join-Path $TerraformDir "*.tf"))) {
        throw "Tidak ada file .tf di '$TerraformDir'. Pindahkan main.tf ke folder infra/ atau pakai -TerraformDir."
    }
    Invoke-Native "terraform init"  { terraform "-chdir=$TerraformDir" init -input=false }
    Invoke-Native "terraform apply" { terraform "-chdir=$TerraformDir" apply -auto-approve -input=false }

    $clusterName = terraform "-chdir=$TerraformDir" output -raw cluster_name
    if ($LASTEXITCODE -ne 0 -or -not $clusterName) {
        throw "Output 'cluster_name' tidak ditemukan. Tambahkan: output ""cluster_name"" { value = civo_kubernetes_cluster.<nama>.name }"
    }
    Invoke-Native "Mengambil kubeconfig" { civo kubernetes config $clusterName --save }
    Invoke-Quiet { kubectl config use-context $clusterName } | Out-Null

    Wait-Until "semua node Ready" 600 {
        $nodes = Invoke-Quiet { kubectl get nodes --no-headers }
        $nodes -and -not ($nodes | Where-Object { $_ -notmatch '\sReady\s' })
    }
    Write-Done "Act -1 selesai: klaster '$clusterName' siap dan terhubung."

    # ---------------------------------------------------------
    # ACT 0: ArgoCD di system-pool
    # ---------------------------------------------------------
    Write-Act "[ACT 0] Menginstal ArgoCD di system-pool..."
    New-Namespace "argocd"
    # --server-side diperlukan karena CRD ArgoCD terlalu besar untuk client-side apply.
    Invoke-Native "Install ArgoCD" {
        kubectl apply -n argocd --server-side --force-conflicts `
            -f https://raw.githubusercontent.com/argoproj/argo-cd/stable/manifests/install.yaml
    }
    Invoke-Native "Menunggu CRD ArgoCD" {
        kubectl wait --for=condition=Established crd/applications.argoproj.io --timeout=120s
    }

    # application-controller adalah StatefulSet, jadi keduanya dipatch.
    $nodeSelectorPatch = @{ spec = @{ template = @{ spec = @{ nodeSelector = $SystemPool } } } }
    $workloads = kubectl get "deployment,statefulset" -n argocd -o name
    foreach ($workload in $workloads) { Invoke-KubectlPatch "argocd" $workload "merge" $nodeSelectorPatch }
    Invoke-KubectlPatch "argocd" "svc/argocd-server" "merge" @{ spec = @{ type = "LoadBalancer" } }
    # Demo hanya memakai Application + login admin: Dex (SSO), notifikasi, dan ApplicationSet dimatikan
    # supaya system-pool tidak kehabisan RAM (pod lain sempat di-evict karena MemoryPressure).
    Invoke-Quiet {
        kubectl scale deployment argocd-dex-server argocd-notifications-controller argocd-applicationset-controller -n argocd --replicas=0
    } | Out-Null
    foreach ($workload in $workloads) {
        Invoke-Native "Rollout $workload" { kubectl rollout status $workload -n argocd --timeout=300s }
    }
    Write-Done "Act 0 selesai: ArgoCD berjalan di system-pool."

    # ---------------------------------------------------------
    # ACT 1: GitOps - ArgoCD memantau k8s/ di repo aplikasi
    # ---------------------------------------------------------
    Write-Act "[ACT 1] Mendaftarkan repo $AppRepo ke ArgoCD..."
    Invoke-Native "Membuat secret repo ArgoCD" {
        kubectl create secret generic repo-ecommerce-app -n argocd `
            --from-literal=type=git --from-literal=url=$AppRepo `
            --from-literal=username=$GitHubUser --from-literal=password=$GitHubToken `
            --dry-run=client -o yaml |
        kubectl label --local -f - -o yaml argocd.argoproj.io/secret-type=repository |
        kubectl apply -f -
    }
    # Pod aplikasi menarik image privat dari GHCR.
    Set-RegistrySecret "default"

    # Kredensial PostgreSQL dibuat SEKALI saja: Postgres hanya membaca POSTGRES_PASSWORD saat volume
    # pertama kali diinisialisasi, jadi mengganti password di run berikutnya akan memutus aplikasi.
    Invoke-Quiet { kubectl get secret postgres-credentials -n default } | Out-Null
    if ($LASTEXITCODE -ne 0) {
        $dbPassword = New-RandomPassword
        Invoke-Native "Membuat secret postgres-credentials" {
            kubectl create secret generic postgres-credentials -n default `
                --from-literal=username=nusamart --from-literal=password=$dbPassword --from-literal=database=nusamart
        }
        Write-Info "secret postgres-credentials dibuat (password acak 32 karakter)."
    } else {
        Write-Info "secret postgres-credentials sudah ada, dipakai ulang."
    }

    # HPA butuh metrics-server (k3s di Civo biasanya sudah menyertakannya).
    Invoke-Quiet { kubectl get apiservice v1beta1.metrics.k8s.io } | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Invoke-Native "Install metrics-server" {
            kubectl apply -f https://github.com/kubernetes-sigs/metrics-server/releases/latest/download/components.yaml
        }
    }

    Invoke-KubectlApply "Membuat ArgoCD Application" @"
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: ecommerce-kcd-demo
  namespace: argocd
spec:
  project: default
  source:
    repoURL: $AppRepo
    targetRevision: $AppBranch
    path: k8s
  destination:
    server: https://kubernetes.default.svc
    namespace: default
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    retry:
      limit: 10
      backoff:
        duration: 15s
        factor: 2
        maxDuration: 5m
"@
    Confirm-CsiReady
    Write-Done "Act 1 selesai: ArgoCD menyinkronkan aplikasi + PostgreSQL + HPA dari folder k8s/ (image muncul setelah build Jenkins pertama)."

    # ---------------------------------------------------------
    # ACT 2: CI & DevSecOps - Jenkins
    # ---------------------------------------------------------
    Write-Act "[ACT 2] Menginstal Jenkins + pipeline NusaMart di system-pool..."
    New-Namespace "jenkins"
    Set-RegistrySecret "jenkins"   # dipakai Kaniko (push) & Trivy (scan) di pod agent
    Invoke-Native "Membuat secret github-creds" {
        kubectl create secret generic github-creds -n jenkins `
            --from-literal=username=$GitHubUser --from-literal=token=$GitHubToken `
            --dry-run=client -o yaml | kubectl apply -f -
    }

    # Here-string kutip tunggal: ${github-creds-*} adalah placeholder JCasC, bukan variabel PowerShell.
    $jenkinsValues = @'
# Tanpa volume: semua konfigurasi Jenkins (job, kredensial) dibuat ulang dari JCasC saat start,
# jadi demo tidak bergantung pada pembuatan volume Civo (yang bisa lambat / kena kuota).
persistence:
  enabled: false
agent:
  # Container jnlp di setiap pod build; default chart (512m/512Mi) terlalu boros untuk node medium.
  resources:
    requests:
      cpu: 100m
      memory: 256Mi
    limits:
      cpu: 500m
      memory: 512Mi
controller:
  nodeSelector:
    kubernetes.civo.com/civo-node-pool: system-pool
  # Request eksplisit agar scheduler tidak menumpuk Jenkins dengan Prometheus di satu node.
  resources:
    requests:
      cpu: 250m
      memory: 1Gi
    limits:
      cpu: "2"
      memory: 1536Mi
  javaOpts: "-XX:MaxRAMPercentage=65"
  serviceType: LoadBalancer
  # Port 80: firewall Civo di infra/main.tf hanya membuka 80-443 untuk publik.
  servicePort: 80
  additionalPlugins:
    - job-dsl
    - pipeline-stage-view
  additionalExistingSecrets:
    - name: github-creds
      keyName: username
    - name: github-creds
      keyName: token
  JCasC:
    configScripts:
      github-credentials: |
        credentials:
          system:
            domainCredentials:
              - credentials:
                  - usernamePassword:
                      scope: GLOBAL
                      id: github-creds
                      description: GitHub (clone + push manifest)
                      username: ${github-creds-username}
                      password: ${github-creds-token}
      ecommerce-job: |
        jobs:
          - script: >
              pipelineJob('ecommerce-app') {
                description('NusaMart: lint, scan, build (Kaniko), Trivy, GitOps deploy')
                properties {
                  pipelineTriggers { triggers { pollSCM { scmpoll_spec('H/2 * * * *') } } }
                }
                definition {
                  cpsScm {
                    scm {
                      git {
                        remote { url('__APP_REPO__'); credentials('github-creds') }
                        branch('*/__APP_BRANCH__')
                      }
                    }
                    scriptPath('Jenkinsfile')
                    lightweight(true)
                  }
                }
              }
'@ -replace '__APP_REPO__', $AppRepo -replace '__APP_BRANCH__', $AppBranch

    Invoke-Native "helm repo add jenkinsci" { helm repo add jenkinsci https://charts.jenkins.io --force-update }
    Install-HelmChart "my-jenkins" "jenkinsci/jenkins" "jenkins" $jenkinsValues
    Write-Done "Act 2 selesai: job 'ecommerce-app' otomatis dibuat dan polling repo tiap 2 menit."

    # ---------------------------------------------------------
    # ACT 3: Observability & Monitoring
    # ---------------------------------------------------------
    Write-Act "[ACT 3] Menginstal Prometheus & Grafana di system-pool..."
    New-Namespace "monitoring"
    $monitoringValues = @'
# Semua komponen diberi request/limit eksplisit: tanpa itu scheduler menumpuknya di satu node
# dan kubelet meng-evict pod saat node kehabisan RAM (MemoryPressure).
prometheusOperator:
  nodeSelector:
    kubernetes.civo.com/civo-node-pool: system-pool
  resources:
    requests: { cpu: 50m, memory: 64Mi }
    limits: { memory: 256Mi }
prometheus:
  prometheusSpec:
    nodeSelector:
      kubernetes.civo.com/civo-node-pool: system-pool
    # Ambil semua ServiceMonitor, termasuk milik aplikasi di namespace default.
    serviceMonitorSelectorNilUsesHelmValues: false
    podMonitorSelectorNilUsesHelmValues: false
    retention: 12h
    resources:
      requests: { cpu: 200m, memory: 512Mi }
      limits: { memory: 1536Mi }
# Alert tidak dipakai di demo; hemat RAM di system-pool.
alertmanager:
  enabled: false
kube-state-metrics:
  nodeSelector:
    kubernetes.civo.com/civo-node-pool: system-pool
  resources:
    requests: { cpu: 25m, memory: 64Mi }
    limits: { memory: 256Mi }
prometheus-node-exporter:
  resources:
    requests: { cpu: 25m, memory: 32Mi }
    limits: { memory: 128Mi }
grafana:
  nodeSelector:
    kubernetes.civo.com/civo-node-pool: system-pool
  resources:
    requests: { cpu: 100m, memory: 192Mi }
    limits: { memory: 512Mi }
  service:
    type: LoadBalancer
  sidecar:
    dashboards:
      # Dashboard NusaMart (k8s/grafana-dashboard.yaml) ada di namespace default.
      searchNamespace: ALL
'@
    Invoke-Native "helm repo add prometheus-community" {
        helm repo add prometheus-community https://prometheus-community.github.io/helm-charts --force-update
    }
    Install-HelmChart "kcd-monitoring" "prometheus-community/kube-prometheus-stack" "monitoring" $monitoringValues
    Write-Done "Act 3 selesai: dashboard 'NusaMart · Live Demo' tersedia di Grafana."
}
catch {
    Write-Host "`n❌ ERROR: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host "`nPod yang belum Running di klaster:" -ForegroundColor DarkYellow
    Invoke-Quiet { kubectl get pods -A } | Select-String -NotMatch "Running|Completed" | Out-Host
    Write-Host "   Perbaiki penyebabnya lalu jalankan ulang skrip; semua langkah aman diulang (idempotent)." -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------
# REKAP AKSES LIVE DEMO
# ---------------------------------------------------------
Write-Host "`n=====================================================" -ForegroundColor Cyan
Write-Host " 🚀 PROVISIONING KCD BANDUNG SELESAI 🚀 " -ForegroundColor Green
Write-Host "=====================================================" -ForegroundColor Cyan

$access = [ordered]@{
    "NusaMart (Rust)" = @{ Url = Get-ServiceAddress "default" "ecommerce-rust-svc";     Login = "-" }
    "ArgoCD"          = @{ Url = Get-ServiceAddress "argocd" "argocd-server" "https"; Login = "admin / " + (Get-SecretValue "argocd" "argocd-initial-admin-secret" "password") }
    "Jenkins"         = @{ Url = Get-ServiceAddress "jenkins" "my-jenkins"; Login = "admin / " + (Get-SecretValue "jenkins" "my-jenkins" "jenkins-admin-password") }
    "Grafana"         = @{ Url = Get-ServiceAddress "monitoring" "kcd-monitoring-grafana"; Login = "admin / " + (Get-SecretValue "monitoring" "kcd-monitoring-grafana" "admin-password") }
}
foreach ($name in $access.Keys) {
    Write-Host ("{0,-16} {1}" -f $name, $access[$name].Url) -ForegroundColor White
    Write-Host ("{0,-16} {1}" -f "", $access[$name].Login) -ForegroundColor Gray
}

Write-Host "`nSkenario live demo:" -ForegroundColor Cyan
Write-Host "  1. Ubah kode (mis. judul hero di static/index.html), commit & push ke '$AppBranch'."
Write-Host "  2. Jenkins: lint + unit/security test + Trivy -> ZAP (DAST) + k6 -> Kaniko -> Trivy image -> update manifest."
Write-Host "  3. ArgoCD sync otomatis -> rolling update tanpa downtime; footer storefront menampilkan build baru."
Write-Host "  4. Checkout beberapa order -> grafik order & omzet di Grafana ikut naik."
Write-Host "  5. Stress test: ./deploy/stress-test.ps1 -Profile stress (atau spike) -> RPS, latensi & jumlah pod naik di Grafana."
Write-Host "     Pantau autoscaling: kubectl get hpa ecommerce-rust -w   |   refresh storefront: nama pod di footer berganti."
Write-Host "(Alamat yang masih <pending> biasanya muncul 1-2 menit kemudian.)" -ForegroundColor DarkGray
