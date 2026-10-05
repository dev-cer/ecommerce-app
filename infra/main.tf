# main.tf
terraform {
  required_providers {
    civo = {
      source  = "civo/civo"
      version = "~> 1.0"
    }
  }
}

provider "civo" {
  region = local.region
}

locals {
  region       = "jkt1" # Region Jakarta
  cluster_name = "ecommerce-kcd-cluster"
  # Dijalankan otomatis saat `terraform destroy` (lihat bagian "Pembersihan otomatis" di bawah).
  cleanup_script = "${path.module}/../deploy/cleanup-civo.ps1"
}

# Windows PowerShell = "powershell"; macOS/Linux: terraform destroy -var powershell=pwsh
variable "powershell" {
  type    = string
  default = "powershell"
}

# 1. Trik Otomatisasi: Deteksi IP Public Laptop Presenter
data "http" "my_public_ip" {
  url = "https://ipv4.icanhazip.com"
}

# 2. Buat Dedicated VPC (Nama baru untuk menghindari error state lama)
resource "civo_network" "custom_vpc" {
  label = "ecommerce-vpc-kcd-v2"
}

# 3. Buat Firewall Rule (Zero-Trust & Web Access)
resource "civo_firewall" "cluster_fw" {
  name                 = "demo-cluster-firewall-v2"
  network_id           = civo_network.custom_vpc.id
  create_default_rules = false

  # Izinkan Kube API dari IP Presenter saja (Keamanan ketat)
  ingress_rule {
    label      = "kubernetes-api"
    protocol   = "tcp"
    port_range = "6443"
    cidr       = ["${chomp(data.http.my_public_ip.response_body)}/32"]
    action     = "allow"
  }

  # Izinkan trafik HTTP/HTTPS agar LoadBalancer (Aplikasi, Jenkins, Grafana) bisa dibuka di browser
  ingress_rule {
    label      = "web-traffic"
    protocol   = "tcp"
    port_range = "80-443"
    cidr       = ["0.0.0.0/0"]
    action     = "allow"
  }
}

# 4. Buat Klaster K3s dengan Cilium CNI & Multi-Pool
resource "civo_kubernetes_cluster" "demo_cluster" {
  name         = local.cluster_name
  network_id   = civo_network.custom_vpc.id
  firewall_id  = civo_firewall.cluster_fw.id
  cluster_type = "k3s"
  cni          = "cilium" # <--- Mengaktifkan Cilium berbasis eBPF secara native

  pools {
    label = "system-pool"
    size  = "g4s.kube.medium"
    # 3 node: k3s control plane + ArgoCD + Jenkins + Prometheus/Grafana tidak muat di 2 x 4 GB
    # (Grafana sempat di-evict karena MemoryPressure). Jumlah node bisa diubah tanpa -replace.
    node_count = 3
  }

  # Klaster baru dihapus SETELAH pembersihan in-cluster, dan sapu network berjalan SETELAH klaster hilang.
  depends_on = [terraform_data.cleanup_network]
}

# App pool: aplikasi (HPA 2-6 pod), PostgreSQL, dan pod build Jenkins (kompilasi Rust butuh ±2-3 GB RAM).
# 1 x small (1 vCPU / 2 GB) terlalu kecil: build bisa OOM dan HPA tidak bisa menambah pod.
# Civo tidak bisa mengubah `size` pool yang sudah ada; ganti pool dengan:
#   terraform apply -replace="civo_kubernetes_node_pool.app_pool"
resource "civo_kubernetes_node_pool" "app_pool" {
  cluster_id = civo_kubernetes_cluster.demo_cluster.id
  label      = "app-pool"
  size       = "g4s.kube.medium"
  node_count = 2

  # Jumlah node app-pool diatur Cluster Autoscaler (2-4 node, lihat deploy/master-demo.ps1);
  # tanpa ini setiap `terraform apply` akan mengembalikannya ke 2 di tengah demo.
  lifecycle {
    ignore_changes = [node_count]
  }
}

# ---------------------------------------------------------------------------------------------
# Pembersihan otomatis saat `terraform destroy`
# LoadBalancer (+ IP publik) dan volume PVC dibuat Kubernetes, bukan Terraform. Tanpa ini, keduanya
# tertinggal (tetap ditagih) dan civo_network tersangkut "Still destroying...". Urutan destroy:
#   cleanup_in_cluster -> app_pool -> klaster -> cleanup_network -> firewall -> network
# ---------------------------------------------------------------------------------------------

# Sebelum klaster dihapus: hapus Service LoadBalancer & PVC dari dalam klaster, tunggu Civo melepasnya.
resource "terraform_data" "cleanup_in_cluster" {
  input = {
    shell        = var.powershell
    script       = local.cleanup_script
    cluster_name = civo_kubernetes_cluster.demo_cluster.name
    cluster_id   = civo_kubernetes_cluster.demo_cluster.id
    region       = local.region
  }
  depends_on = [civo_kubernetes_node_pool.app_pool]

  # Jangan pernah diganti saat `apply` biasa: penggantian = provisioner destroy berjalan = LB terhapus.
  lifecycle {
    ignore_changes = all
  }

  provisioner "local-exec" {
    when        = destroy
    on_failure  = continue
    interpreter = [self.input.shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]
    command     = "& '${self.input.script}' -Phase InCluster -ClusterName '${self.input.cluster_name}' -ClusterId '${self.input.cluster_id}' -Region '${self.input.region}'"
  }
}

# Setelah klaster dihapus, sebelum network: sapu LB/volume/firewall lain yang masih menempel di network.
resource "terraform_data" "cleanup_network" {
  input = {
    shell        = var.powershell
    script       = local.cleanup_script
    cluster_name = local.cluster_name
    network_id   = civo_network.custom_vpc.id
    firewall_id  = civo_firewall.cluster_fw.id # dihapus Terraform sendiri, jangan disapu
    region       = local.region
  }

  lifecycle {
    ignore_changes = all
  }

  provisioner "local-exec" {
    when        = destroy
    on_failure  = continue
    interpreter = [self.input.shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]
    command     = "& '${self.input.script}' -Phase Sweep -ClusterName '${self.input.cluster_name}' -NetworkId '${self.input.network_id}' -KeepIds '${self.input.firewall_id}' -Region '${self.input.region}'"
  }
}

# 5. OUTPUT (Wajib agar PowerShell otomatis mendapatkan nama klaster)
output "cluster_name" {
  value       = civo_kubernetes_cluster.demo_cluster.name
  description = "Nama klaster untuk ditarik secara otomatis oleh skrip Act 0"
}

# Dipakai Cluster Autoscaler (deploy/master-demo.ps1) untuk menambah/mengurangi node app-pool.
output "cluster_id" {
  value = civo_kubernetes_cluster.demo_cluster.id
}

# Dipakai deploy/destroy-demo.ps1 untuk menyapu sisa resource (LB, volume, firewall) di network ini.
output "network_id" {
  value = civo_network.custom_vpc.id
}

output "region" {
  value = civo_kubernetes_cluster.demo_cluster.region
}
