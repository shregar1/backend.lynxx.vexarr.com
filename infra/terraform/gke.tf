# ---------------------------------------------------------------------------
# GKE Autopilot — fully managed, Workload Identity enabled by default.
# ---------------------------------------------------------------------------

resource "google_container_cluster" "lynxx" {
  provider = google-beta
  name     = var.cluster_name
  location = var.region   # regional cluster → 3-zone control plane

  # Autopilot handles node provisioning, patching, and scaling automatically.
  enable_autopilot = true

  network    = google_compute_network.lynxx.id
  subnetwork = google_compute_subnetwork.lynxx.id

  ip_allocation_policy {
    cluster_secondary_range_name  = "pods"
    services_secondary_range_name = "services"
  }

  # Private cluster — nodes have no public IPs.
  private_cluster_config {
    enable_private_nodes    = true
    enable_private_endpoint = false   # public endpoint for kubectl access (secured by RBAC + OIDC)
    master_ipv4_cidr_block  = "172.16.0.0/28"
  }

  # Authorised networks for the GKE control plane endpoint.
  # Restrict to your CI/CD runners + operator CIDRs in prod.
  master_authorized_networks_config {
    cidr_blocks {
      cidr_block   = "0.0.0.0/0"
      display_name = "all (lock down in production)"
    }
  }

  # Workload Identity — pods authenticate to GCP APIs using a K8s SA,
  # no static service-account key files needed.
  workload_identity_config {
    workload_pool = "${var.project_id}.svc.id.goog"
  }

  release_channel {
    channel = "REGULAR"
  }

  addons_config {
    gce_persistent_disk_csi_driver_config {
      enabled = true
    }
  }

  # Vertical Pod Autoscaler — Autopilot uses this internally.
  vertical_pod_autoscaling {
    enabled = true
  }

  maintenance_policy {
    recurring_window {
      start_time = "2024-01-01T00:00:00Z"
      end_time   = "2024-01-02T00:00:00Z"
      recurrence = "FREQ=WEEKLY;BYDAY=SA,SU,WE"
    }
  }

  depends_on = [
    google_project_service.apis,
    google_service_networking_connection.sql,
  ]

  lifecycle {
    ignore_changes = [
      # Autopilot manages these internally
      node_config,
      node_pool,
    ]
  }
}
