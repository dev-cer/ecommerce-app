"""Generator dashboard Grafana "Civo Quota & Kapasitas Klaster" -> k8s/grafana-quota-dashboard.yaml.

Jalankan ulang setelah mengubah kuota akun (QUOTA) atau tata letak:
    python3 deploy/dashboards/civo_quota.py
"""

import json
from pathlib import Path

# Kuota akun Civo (halaman Settings -> Quota). Sesuaikan bila Civo menaikkan kuota.
QUOTA = {
    "instances": 32,
    "cpus": 64,
    "ram_mb": 268_768,
    "disk_gb": 2000,
    "volumes": 16,
    "load_balancers": 16,
}

DS = {"type": "prometheus", "uid": "prometheus"}
POOL_LABEL = "label_kubernetes_civo_com_civo_node_pool"

# Pod yang benar-benar memakai jatah node (pod Succeeded/Failed tidak dihitung scheduler).
ACTIVE = 'max by (namespace, pod) (kube_pod_status_phase{phase=~"Pending|Running"} == 1)'
RUNNING = 'max by (namespace, pod) (kube_pod_status_phase{phase="Running"} == 1)'
POD_NODE = 'max by (namespace, pod, node) (kube_pod_info{node!=""})'
POOL_NODES = f'max by (node) (kube_node_labels{{{POOL_LABEL}="$pool"}})'
CPU_USED = 'sum by (namespace, pod) (rate(container_cpu_usage_seconds_total{container!="", image!=""}[2m]))'
MEM_USED = 'sum by (namespace, pod) (container_memory_working_set_bytes{container!="", image!=""})'


def requests(resource: str, pool: bool = False) -> str:
    expr = f'kube_pod_container_resource_requests{{resource="{resource}"}} * on (namespace, pod) group_left () {ACTIVE}'
    if pool:
        expr = f"({expr}) * on (node) group_left () {POOL_NODES}"
    return f"sum({expr})"


def allocatable(resource: str, pool: bool = False) -> str:
    expr = f'kube_node_status_allocatable{{resource="{resource}"}}'
    if pool:
        expr = f"{expr} * on (node) group_left () {POOL_NODES}"
    return f"sum({expr})"


def used(per_pod: str, pool: bool = False) -> str:
    if not pool:
        return f"sum({per_pod})"
    return f"sum({per_pod} * on (namespace, pod) group_left (node) {POD_NODE} * on (node) group_left () {POOL_NODES})"


def running_pods(pool: bool = False) -> str:
    expr = f"{POD_NODE} * on (namespace, pod) group_left () {RUNNING}"
    if pool:
        expr = f"{expr} * on (node) group_left () {POOL_NODES}"
    return f"count({expr})"


def pct(numerator: str, denominator: str) -> str:
    return f"100 * ({numerator}) / ({denominator})"


# ---------------------------------------------------------------- panel builders

_ids = iter(range(1, 1000))
FREE_STEPS = [
    {"color": "#3fb27f", "value": None},  # hijau Civo
    {"color": "#f2a33a", "value": 75},
    {"color": "#e5484d", "value": 90},  # merah seperti "Public IP addresses" di Civo
]


def gauge(title, expr, x, y, w=6, h=7, unit="none", maximum=None, percent=False, decimals=None, description=""):
    defaults = {
        "unit": "percent" if percent else unit,
        "min": 0,
        "color": {"mode": "thresholds"},
        "thresholds": {
            # Kuota: ambang relatif terhadap batas (75% / 90% terpakai).
            "mode": "absolute" if percent else "percentage",
            "steps": FREE_STEPS,
        },
    }
    if percent:
        defaults["max"] = 100
    elif maximum is not None:
        defaults["max"] = maximum
    if decimals is not None:
        defaults["decimals"] = decimals
    return {
        "id": next(_ids),
        "type": "gauge",
        "title": title,
        "description": description,
        "datasource": DS,
        "gridPos": {"h": h, "w": w, "x": x, "y": y},
        "targets": [{"refId": "A", "datasource": DS, "expr": expr, "instant": True}],
        "fieldConfig": {"defaults": defaults, "overrides": []},
        "options": {
            "reduceOptions": {"calcs": ["lastNotNull"], "fields": "", "values": False},
            "showThresholdMarkers": False,
            "showThresholdLabels": False,
            "sizing": "auto",
            "minVizHeight": 120,
            "minVizWidth": 120,
            "text": {"valueSize": 40},
        },
    }


def stat(title, expr, x, y, w=6, h=7):
    """Angka besar tanpa batas (gauge tanpa max akan selalu tampil penuh/merah)."""
    return {
        "id": next(_ids),
        "type": "stat",
        "title": title,
        "datasource": DS,
        "gridPos": {"h": h, "w": w, "x": x, "y": y},
        "targets": [{"refId": "A", "datasource": DS, "expr": expr, "instant": True}],
        "fieldConfig": {
            "defaults": {"unit": "none", "decimals": 0, "color": {"mode": "fixed", "fixedColor": "#3fb27f"}},
            "overrides": [],
        },
        "options": {
            "reduceOptions": {"calcs": ["lastNotNull"], "fields": "", "values": False},
            "colorMode": "value",
            "graphMode": "none",
            "textMode": "value",
            "justifyMode": "center",
            "text": {"valueSize": 64},
        },
    }


def row(title, y, repeat=None):
    panel = {"id": next(_ids), "type": "row", "title": title, "collapsed": False, "gridPos": {"h": 1, "w": 24, "x": 0, "y": y}, "panels": []}
    if repeat:
        panel["repeat"] = repeat
    return panel


q = QUOTA
panels = [
    row("Kuota akun Civo · pemakaian klaster ini (batas = kuota akun di Civo → Settings → Quota)", 0),
    gauge(f"Instances ({q['instances']})", "count(kube_node_info)", 0, 1, maximum=q["instances"],
          description="Jumlah node klaster ini dibanding kuota instance akun Civo."),
    gauge(f"CPUs ({q['cpus']})", 'sum(kube_node_status_capacity{resource="cpu"})', 6, 1, maximum=q["cpus"],
          description="Total vCPU node klaster ini dibanding kuota CPU akun."),
    gauge(f"RAM ({round(q['ram_mb'] / 1024)}GB)", 'sum(kube_node_status_capacity{resource="memory"})', 12, 1,
          unit="bytes", maximum=q["ram_mb"] * 1024 * 1024, decimals=0,
          description="Total RAM node klaster ini dibanding kuota RAM akun."),
    gauge(f"Disk ({q['disk_gb']}GB)", 'sum(node_filesystem_size_bytes{mountpoint="/", fstype!~"tmpfs|overlay"}) + (sum(kube_persistentvolume_capacity_bytes) or vector(0))',
          18, 1, unit="decbytes", maximum=q["disk_gb"] * 10**9, decimals=0,
          description="Disk node + volume (PVC) klaster ini dibanding kuota disk akun."),
    gauge(f"Volumes ({q['volumes']})", "count(kube_persistentvolume_info) or vector(0)", 0, 8, maximum=q["volumes"],
          description="Jumlah volume Civo (PersistentVolume) milik klaster ini."),
    gauge(f"Load Balancers ({q['load_balancers']})", 'count(kube_service_spec_type{type="LoadBalancer"}) or vector(0)', 6, 8,
          maximum=q["load_balancers"], description="Service type LoadBalancer = 1 load balancer + 1 IP publik Civo."),
    gauge("Pods berjalan / kapasitas", pct(running_pods(), allocatable("pods")), 12, 8, percent=True, decimals=0,
          description="Pod Running dibanding kapasitas pod seluruh node."),
    gauge("Disk node terpakai", '100 * (1 - sum(node_filesystem_avail_bytes{mountpoint="/", fstype!~"tmpfs|overlay"}) / sum(node_filesystem_size_bytes{mountpoint="/", fstype!~"tmpfs|overlay"}))',
          18, 8, percent=True, decimals=0, description="Pemakaian disk root seluruh node."),

    row("Kapasitas klaster (live) · dipesan = request pod, dipakai = pemakaian nyata", 15),
    gauge("CPU dipesan", pct(requests("cpu"), allocatable("cpu")), 0, 16, percent=True, decimals=0,
          description="Jumlah CPU request pod aktif dibanding allocatable. Mendekati 100% = pod baru Pending."),
    gauge("CPU dipakai", pct(used(CPU_USED), allocatable("cpu")), 6, 16, percent=True, decimals=0),
    gauge("RAM dipesan", pct(requests("memory"), allocatable("memory")), 12, 16, percent=True, decimals=0,
          description="Mendekati 100% = pod baru 'Insufficient memory'."),
    gauge("RAM dipakai", pct(used(MEM_USED), allocatable("memory")), 18, 16, percent=True, decimals=0,
          description="Mendekati 100% = node MemoryPressure dan pod di-evict."),

    # Satu baris per node pool (system-pool, app-pool) lewat repeat variabel $pool.
    row("Node pool: $pool", 23, repeat="pool"),
    stat("Nodes", f"count({POOL_NODES})", 0, 24, w=4),
    gauge("CPU dipesan", pct(requests("cpu", True), allocatable("cpu", True)), 4, 24, w=4, percent=True, decimals=0),
    gauge("CPU dipakai", pct(used(CPU_USED, True), allocatable("cpu", True)), 8, 24, w=4, percent=True, decimals=0),
    gauge("RAM dipesan", pct(requests("memory", True), allocatable("memory", True)), 12, 24, w=4, percent=True, decimals=0),
    gauge("RAM dipakai", pct(used(MEM_USED, True), allocatable("memory", True)), 16, 24, w=4, percent=True, decimals=0),
    gauge("Pods", pct(running_pods(True), allocatable("pods", True)), 20, 24, w=4, percent=True, decimals=0),
]

dashboard = {
    "title": "Civo Quota & Kapasitas Klaster",
    "uid": "civo-quota",
    "tags": ["kcd", "civo", "capacity"],
    "timezone": "browser",
    "refresh": "10s",
    "time": {"from": "now-15m", "to": "now"},
    "schemaVersion": 39,
    "graphTooltip": 0,
    "templating": {
        "list": [
            {
                "name": "pool",
                "label": "Node pool",
                "type": "query",
                "datasource": DS,
                "query": {"query": f"label_values(kube_node_labels, {POOL_LABEL})", "refId": "pool"},
                "definition": f"label_values(kube_node_labels, {POOL_LABEL})",
                "refresh": 2,
                "multi": True,
                "includeAll": True,
                "current": {"selected": True, "text": ["All"], "value": ["$__all"]},
                "sort": 1,
            }
        ]
    },
    "panels": panels,
}

if __name__ == "__main__":
    body = json.dumps(dashboard, indent=2, ensure_ascii=False)
    out = Path(__file__).resolve().parents[2] / "k8s" / "grafana-quota-dashboard.yaml"
    out.write_text(
        "# Dashboard \"Civo Quota & Kapasitas Klaster\" (dibuat oleh deploy/dashboards/civo_quota.py, jangan edit manual).\n"
        "# Dimuat otomatis oleh sidecar Grafana kube-prometheus-stack (label grafana_dashboard).\n"
        "apiVersion: v1\n"
        "kind: ConfigMap\n"
        "metadata:\n"
        "  name: civo-quota-dashboard\n"
        "  labels:\n"
        '    grafana_dashboard: "1"\n'
        "data:\n"
        "  civo-quota.json: |\n" + "\n".join("    " + line for line in body.splitlines()) + "\n"
    )
    print(f"ditulis: {out}")
