<#
.SYNOPSIS
    Menjalankan stress test k6 dari DALAM klaster terhadap NusaMart, untuk live demo bersama dashboard Grafana.

.DESCRIPTION
    Script k6 (tests/load/stress.js) dimuat sebagai ConfigMap lalu dijalankan sebagai Job di system-pool,
    sehingga beban tidak dibatasi bandwidth laptop dan grafik CPU aplikasi di Grafana tetap bersih.

.EXAMPLE
    ./stress-test.ps1                 # profil stress (±4 menit, sampai 300 virtual user)
    ./stress-test.ps1 -Profile spike  # lonjakan mendadak 400 virtual user
    ./stress-test.ps1 -Profile smoke  # cek cepat 30 detik
#>
[CmdletBinding()]
param(
    [Alias("Profile")]
    [ValidateSet("smoke", "stress", "spike")]
    [string]$LoadProfile = "stress",
    [string]$Target = "http://ecommerce-rust-svc.default.svc.cluster.local",
    [string]$Namespace = "loadtest"
)

$ErrorActionPreference = "Stop"
$JobName = "k6-$LoadProfile"

function Invoke-Native([string]$Description, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Description gagal (exit code $LASTEXITCODE)." }
}

try {
    $scriptPath = Join-Path $PSScriptRoot "../tests/load/stress.js"
    if (-not (Test-Path $scriptPath)) { throw "Script k6 tidak ditemukan: $scriptPath" }

    Write-Host "🔥 Stress test NusaMart · profil '$LoadProfile' · target $Target" -ForegroundColor Cyan

    Invoke-Native "Membuat namespace" {
        kubectl create namespace $Namespace --dry-run=client -o yaml | kubectl apply -f -
    }
    Invoke-Native "Mengunggah script k6" {
        kubectl create configmap k6-script -n $Namespace --from-file=stress.js=$scriptPath --dry-run=client -o yaml |
            kubectl apply -f -
    }
    Invoke-Native "Menghapus job lama" { kubectl delete job $JobName -n $Namespace --ignore-not-found --wait=true }

    $job = @"
apiVersion: batch/v1
kind: Job
metadata:
  name: $JobName
  namespace: $Namespace
spec:
  backoffLimit: 0
  ttlSecondsAfterFinished: 900
  template:
    metadata:
      labels:
        app: k6
    spec:
      restartPolicy: Never
      nodeSelector:
        kubernetes.civo.com/civo-node-pool: system-pool
      containers:
        - name: k6
          image: grafana/k6:2.3.0
          args: ["run", "-e", "BASE_URL=$Target", "-e", "PROFILE=$LoadProfile", "/scripts/stress.js"]
          resources:
            requests: { cpu: 500m, memory: 256Mi }
            limits: { cpu: "2", memory: 1Gi }
          volumeMounts:
            - name: script
              mountPath: /scripts
      volumes:
        - name: script
          configMap:
            name: k6-script
"@
    $file = Join-Path ([IO.Path]::GetTempPath()) "$JobName.yaml"
    [IO.File]::WriteAllText($file, $job, (New-Object Text.UTF8Encoding $false))
    try { Invoke-Native "Menjalankan job k6" { kubectl apply -f $file } } finally { Remove-Item $file -ErrorAction SilentlyContinue }

    Write-Host "⏳ Menunggu pod k6 berjalan... (buka Grafana → dashboard 'NusaMart · Live Demo')" -ForegroundColor DarkGray
    $deadline = (Get-Date).AddMinutes(3)
    do {
        Start-Sleep -Seconds 3
        $phase = kubectl get pods -n $Namespace -l job-name=$JobName -o "jsonpath={.items[0].status.phase}" 2>$null
        if ((Get-Date) -gt $deadline) { throw "Pod k6 tidak kunjung berjalan (phase: $phase)." }
    } while (-not $phase -or $phase -eq "Pending")

    kubectl logs -f "job/$JobName" -n $Namespace

    # Status job butuh sesaat untuk ter-update setelah log selesai.
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Seconds 2
        $succeeded = kubectl get job $JobName -n $Namespace -o "jsonpath={.status.succeeded}" 2>$null
        $failed = kubectl get job $JobName -n $Namespace -o "jsonpath={.status.failed}" 2>$null
    } while (-not $succeeded -and -not $failed -and (Get-Date) -lt $deadline)

    if ($succeeded -eq "1") {
        Write-Host "`n✅ Semua threshold k6 terpenuhi: aplikasi lolos profil '$LoadProfile'." -ForegroundColor Green
    } else {
        Write-Host "`n⚠️  Ada threshold k6 yang terlewati. Cek ringkasan di atas & panel latensi di Grafana." -ForegroundColor Yellow
        exit 1
    }
}
catch {
    Write-Host "❌ ERROR: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
