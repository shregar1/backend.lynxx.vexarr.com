# ---------------------------------------------------------------------------
# Cloud Run — website (lynxx.dev) and dashboard (app.lynxx.dev).
# Both are static SPAs served by nginx inside the container.
#
# Initial image is a placeholder — the CI/CD pipeline (deploy-gcp.yml)
# deploys the real images on every push to main.
# ---------------------------------------------------------------------------

locals {
  placeholder_image = "us-docker.pkg.dev/cloudrun/container/hello:latest"
}

resource "google_cloud_run_v2_service" "website" {
  name     = "lynxx-website"
  location = var.region

  template {
    containers {
      image = local.placeholder_image

      ports {
        container_port = 8080
      }

      resources {
        limits = {
          cpu    = "1"
          memory = "256Mi"
        }
        cpu_idle = true   # scale to zero when no traffic
      }
    }

    scaling {
      min_instance_count = 0
      max_instance_count = 10
    }
  }

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  # CI/CD owns image and template updates — Terraform only manages the service scaffold.
  lifecycle {
    ignore_changes = [
      template,
      client,
      client_version,
    ]
  }

  depends_on = [google_project_service.apis]
}

resource "google_cloud_run_v2_service" "dashboard" {
  name     = "lynxx-dashboard"
  location = var.region

  template {
    containers {
      image = local.placeholder_image

      ports {
        container_port = 8080
      }

      resources {
        limits = {
          cpu    = "1"
          memory = "256Mi"
        }
        cpu_idle = true
      }
    }

    scaling {
      min_instance_count = 0
      max_instance_count = 10
    }
  }

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  lifecycle {
    ignore_changes = [
      template,
      client,
      client_version,
    ]
  }

  depends_on = [google_project_service.apis]
}

# ---------------------------------------------------------------------------
# Billing / licence service (api.lynxx.dev). Hosts:
#   /v1/auth/*, /v1/account/*, /v1/license/verify, /v1/billing/*, /v1/stripe/webhook
# Connects to Cloud SQL via the built-in Cloud Run sql_connections; reads
# session and Stripe secrets from Secret Manager. Public ingress; auth is
# handled in-app via session tokens + Stripe signature verification.
# ---------------------------------------------------------------------------

resource "google_cloud_run_v2_service" "billing" {
  name     = "lynxx-billing"
  location = var.region

  template {
    service_account = google_service_account.gateway.email

    scaling {
      min_instance_count = 1   # keep one warm so Stripe webhooks never cold-start
      max_instance_count = 10
    }

    # Direct VPC egress so the service can reach Cloud SQL via its
    # private IP. PRIVATE_RANGES_ONLY keeps internet egress off the VPC
    # path (cheaper, faster) — the Stripe HTTP client still goes out the
    # default Cloud Run NAT.
    vpc_access {
      network_interfaces {
        network    = google_compute_network.lynxx.id
        subnetwork = google_compute_subnetwork.lynxx.id
      }
      egress = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image = local.placeholder_image

      ports {
        container_port = 9100
      }

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
        cpu_idle = false
      }

      env {
        # Plain TCP to Cloud SQL's private IP. Reaches the database over
        # the VPC via direct egress (configured below). Simpler than
        # Cloud Run's Unix-socket integration and avoids a sqlx URL
        # parsing rabbit hole.
        name  = "DATABASE_URL"
        value = "postgres://lynxx:${var.db_password}@${google_sql_database_instance.lynxx.private_ip_address}:5432/lynxx"
      }
      env {
        name  = "LYNXX_BILLING_BIND"
        value = "0.0.0.0:9100"
      }
      env {
        name  = "WEBSITE_ORIGIN"
        value = "https://${var.website_domain}"
      }
      env {
        name  = "STRIPE_SUCCESS_URL"
        value = "https://${var.website_domain}/account/api-keys"
      }
      env {
        name  = "STRIPE_CANCEL_URL"
        value = "https://${var.website_domain}/pricing"
      }
      env {
        name  = "STRIPE_PORTAL_RETURN_URL"
        value = "https://${var.website_domain}/account/billing"
      }
      env {
        name = "LYNXX_SESSION_SECRET"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.session_secret.secret_id
            version = "latest"
          }
        }
      }
      env {
        name = "LYNXX_ADMIN_API_KEY"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.admin_api_key.secret_id
            version = "latest"
          }
        }
      }
      env {
        name = "STRIPE_SECRET_KEY"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.stripe_secret_key.secret_id
            version = "latest"
          }
        }
      }
      env {
        name = "STRIPE_WEBHOOK_SECRET"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.stripe_webhook_secret.secret_id
            version = "latest"
          }
        }
      }
      env {
        name  = "STRIPE_PRICE_PRO"
        value = var.stripe_price_pro
      }
      env {
        name  = "STRIPE_PRICE_BUSINESS"
        value = var.stripe_price_business
      }
      env {
        name  = "BINARIES_BUCKET"
        value = google_storage_bucket.binaries.name
      }
    }
  }

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  # Image rolls forward via CI/CD; only the Terraform-managed scaffold
  # (env list, scaling, SA, sql connection) is reconciled here.
  lifecycle {
    ignore_changes = [
      template[0].containers[0].image,
      client,
      client_version,
    ]
  }

  depends_on = [
    google_project_service.apis,
    google_secret_manager_secret_version.db_url,
  ]
}

# Allow unauthenticated access to all three public services.
resource "google_cloud_run_v2_service_iam_member" "website_public" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.website.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

resource "google_cloud_run_v2_service_iam_member" "dashboard_public" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.dashboard.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

resource "google_cloud_run_v2_service_iam_member" "billing_public" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.billing.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

# ---------------------------------------------------------------------------
# Custom domain — api.lynxx.vexarr.com → lynxx-billing
# Requires the parent domain (vexarr.com or lynxx.vexarr.com) to be
# verified in Google Search Console under the same Google account that
# owns this project. Output `billing_domain_dns` lists the DNS records
# to add at the domain's authoritative DNS provider.
# ---------------------------------------------------------------------------

resource "google_cloud_run_domain_mapping" "billing" {
  name     = var.domain
  location = var.region

  metadata {
    namespace = var.project_id
  }

  spec {
    route_name = google_cloud_run_v2_service.billing.name
  }

  depends_on = [google_cloud_run_v2_service.billing]
}
