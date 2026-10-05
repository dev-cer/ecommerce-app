# infra/

Taruh konfigurasi Terraform Civo (`main.tf`) di folder ini. `deploy/master-demo.ps1` otomatis
memakai folder ini (bisa diganti dengan `-TerraformDir`).

Syarat `main.tf`:

- Token Civo **tidak** ditulis di file; pakai environment variable `CIVO_TOKEN`.
- Node pool untuk ArgoCD/Jenkins/monitoring bernama `system-pool`.
- Punya output nama klaster:

  ```hcl
  output "cluster_name" {
    value = civo_kubernetes_cluster.<nama_resource>.name
  }
  ```

Yang boleh di-commit: `main.tf` (dan file `.tf` lain) serta `.terraform.lock.hcl`.
`terraform.tfstate`, folder `.terraform/`, dan `*.tfvars` sudah di-ignore karena berisi data rahasia klaster.

Saat `terraform destroy`, provisioner `terraform_data.cleanup_in_cluster` dan `cleanup_network` otomatis menjalankan
`../deploy/cleanup-civo.ps1` agar LoadBalancer, IP publik, dan volume yang dibuat Kubernetes ikut terhapus
(network tidak tersangkut). Butuh `civo` CLI yang sudah login dan `kubectl`. Di macOS/Linux:
`terraform destroy -var powershell=pwsh`.
