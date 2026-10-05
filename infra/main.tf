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
  region = "jkt1" # Region Jakarta
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
  name         = "ecommerce-kcd-cluster"
  network_id   = civo_network.custom_vpc.id
  firewall_id  = civo_firewall.cluster_fw.id
  cluster_type = "k3s"
  cni          = "cilium" # <--- Mengaktifkan Cilium berbasis eBPF secara native
  
  pools {
    label      = "system-pool"
    size       = "g4s.kube.medium"
    node_count = 2 
  }
}

resource "civo_kubernetes_node_pool" "app_pool" {
  cluster_id = civo_kubernetes_cluster.demo_cluster.id
  label      = "app-pool"
  size       = "g4s.kube.small"
  node_count = 1 
}

# 5. OUTPUT (Wajib agar PowerShell otomatis mendapatkan nama klaster)
output "cluster_name" {
  value       = civo_kubernetes_cluster.demo_cluster.name
  description = "Nama klaster untuk ditarik secara otomatis oleh skrip Act 0"
}