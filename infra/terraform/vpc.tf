# ---------------------------------------------------------------------------
# VPC — one private network, one subnet, Cloud NAT for egress.
# Cloud SQL uses private services access (RFC 1918 peering).
# ---------------------------------------------------------------------------

resource "google_compute_network" "lynxx" {
  name                    = "lynxx-vpc"
  auto_create_subnetworks = false
  depends_on              = [google_project_service.apis]
}

resource "google_compute_subnetwork" "lynxx" {
  name                     = "lynxx-subnet"
  region                   = var.region
  network                  = google_compute_network.lynxx.id
  ip_cidr_range            = "10.10.0.0/20"
  private_ip_google_access = true

  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = "10.20.0.0/16"
  }
  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = "10.30.0.0/20"
  }
}

# Cloud Router + NAT — lets private GKE nodes pull images / reach the internet.
resource "google_compute_router" "lynxx" {
  name    = "lynxx-router"
  region  = var.region
  network = google_compute_network.lynxx.id
}

resource "google_compute_router_nat" "lynxx" {
  name                               = "lynxx-nat"
  router                             = google_compute_router.lynxx.name
  region                             = var.region
  nat_ip_allocate_option             = "AUTO_ONLY"
  source_subnetwork_ip_ranges_to_nat = "ALL_SUBNETWORKS_ALL_IP_RANGES"
}

# ---------------------------------------------------------------------------
# Private services access for Cloud SQL has been retired — the lynxx
# product now shares the `vexarr-com` Cloud SQL instance and reaches it
# via the Cloud Run Auth Proxy sidecar (TCP 5432 over a Unix socket
# mounted by Cloud Run from /cloudsql/<connection-name>). No VPC peering
# range or service-networking connection is required.
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Firewall — allow health-checks from the GCP LB prober ranges
# ---------------------------------------------------------------------------
resource "google_compute_firewall" "allow_lb_health_check" {
  name    = "lynxx-allow-lb-hc"
  network = google_compute_network.lynxx.id

  allow {
    protocol = "tcp"
    ports    = ["9000", "9001"]
  }

  # GCP health check source ranges
  source_ranges = ["130.211.0.0/22", "35.191.0.0/16"]
  target_tags   = ["lynxx-gateway"]
}
