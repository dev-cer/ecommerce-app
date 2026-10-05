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
