# ---------------------------------------------------------------------------
# Cloud Run — website (lynxx.vexarr.com) and dashboard
# (app.lynxx.vexarr.com). Both are static SPAs served by a small static
# file server inside each container image.
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
# Billing / licence service (api.lynxx.vexarr.com). Hosts:
#   /v1/auth/*, /v1/account/*, /v1/license/verify, /v1/billing/*, /v1/stripe/webhook
# Connects to the shared Cloud SQL instance (`vexarr-com`) via Cloud Run's
# built-in Auth Proxy: a Unix socket at /cloudsql/<connection-name>/.s.PGSQL.5432.
# Stripe + session secrets come from Secret Manager. Public ingress; auth is
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

    # Cloud Run's Cloud SQL integration: mounts a Unix socket at
    # /cloudsql/<connection-name> and authenticates via the runtime SA.
    # No VPC peering or private IP required — works with the shared
    # `vexarr-com` instance which sits outside the lynxx VPC.
    volumes {
      name = "cloudsql"
      cloud_sql_instance {
        instances = [data.google_sql_database_instance.shared.connection_name]
      }
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

      volume_mounts {
        name       = "cloudsql"
        mount_path = "/cloudsql"
      }

      # All non-trivial config lives in Secret Manager — that gives us
      # one place to look up production values, audit changes, and
      # rotate without redeploying. The only literal here is the bind
      # address, which is internal to the container.
      env {
        name  = "LYNXX_BILLING_BIND"
        value = "0.0.0.0:9100"
      }
      # ----- Database --------------------------------------------------------
      env {
        name = "DB_HOST"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_DB_HOST"
            version = "latest"
          }
        }
      }
      env {
        name = "DB_USER"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_DB_USER"
            version = "latest"
          }
        }
      }
      env {
        name = "DB_NAME"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_DB_NAME"
            version = "latest"
          }
        }
      }
      env {
        name = "DB_PASSWORD"
        value_source {
          secret_key_ref {
            secret  = data.google_secret_manager_secret.lynxx_db_password.secret_id
            version = "latest"
          }
        }
      }
      # ----- Public website + Stripe redirect targets ------------------------
      env {
        name = "WEBSITE_ORIGIN"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_WEBSITE_ORIGIN"
            version = "latest"
          }
        }
      }
      env {
        name = "STRIPE_SUCCESS_URL"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_STRIPE_SUCCESS_URL"
            version = "latest"
          }
        }
      }
      env {
        name = "STRIPE_CANCEL_URL"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_STRIPE_CANCEL_URL"
            version = "latest"
          }
        }
      }
      env {
        name = "STRIPE_PORTAL_RETURN_URL"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_STRIPE_PORTAL_RETURN_URL"
            version = "latest"
          }
        }
      }
      # ----- App secrets -----------------------------------------------------
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
        name = "LYNXX_ALLOWED_ORIGINS"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_ALLOWED_ORIGINS"
            version = "latest"
          }
        }
      }
      # ----- Stripe ----------------------------------------------------------
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
        name = "STRIPE_PRICE_PRO"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_STRIPE_PRICE_PRO"
            version = "latest"
          }
        }
      }
      env {
        name = "STRIPE_PRICE_BUSINESS"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_STRIPE_PRICE_BUSINESS"
            version = "latest"
          }
        }
      }
      # ----- Binaries bucket -------------------------------------------------
      env {
        name = "BINARIES_BUCKET"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_BINARIES_BUCKET"
            version = "latest"
          }
        }
      }
      env {
        name = "BINARIES_PREFIX"
        value_source {
          secret_key_ref {
            secret  = "LYNXX_BINARIES_PREFIX"
            version = "latest"
          }
        }
      }
      # OAuth Client ID for Sign-in with Google. Treated as a secret
      # only for central management — the value is public and the
      # website ships the same string in its JS bundle.
      env {
        name = "GOOGLE_OAUTH_CLIENT_ID"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.google_oauth_client_id.secret_id
            version = "latest"
          }
        }
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

# ---------------------------------------------------------------------------
# Custom domain — get.lynxx.vexarr.com → lynxx-billing
# Dedicated host for the installer so the customer-facing command is
# `curl -fsSL https://get.lynxx.vexarr.com | sh` (matching the
# get.docker.com / sh.rustup.rs convention). The billing service
# answers `/` with the install script, plus the existing
# /install + /install.sh + /v1/install/binary endpoints.
# ---------------------------------------------------------------------------

resource "google_cloud_run_domain_mapping" "get" {
  name     = "get.${var.website_domain}"
  location = var.region

  metadata {
    namespace = var.project_id
  }

  spec {
    route_name = google_cloud_run_v2_service.billing.name
  }

  depends_on = [google_cloud_run_v2_service.billing]
}

# ---------------------------------------------------------------------------
# Custom domain — lynxx.vexarr.com → lynxx-website
# Requires `lynxx.vexarr.com` (or `vexarr.com`) to be verified in
# Google Search Console under the same Google account that owns this
# project. The output `website_domain_dns` lists the DNS records to
# add at the authoritative DNS provider.
# ---------------------------------------------------------------------------

resource "google_cloud_run_domain_mapping" "website" {
  name     = var.website_domain
  location = var.region

  metadata {
    namespace = var.project_id
  }

  spec {
    route_name = google_cloud_run_v2_service.website.name
  }

  depends_on = [google_cloud_run_v2_service.website]
}

# ---------------------------------------------------------------------------
# Custom domain — app.lynxx.vexarr.com → lynxx-dashboard
# Same prerequisites as the website domain. The dashboard is currently
# a placeholder image; the mapping reserves the hostname for once the
# real customer dashboard ships.
# ---------------------------------------------------------------------------

resource "google_cloud_run_domain_mapping" "dashboard" {
  name     = var.dashboard_domain
  location = var.region

  metadata {
    namespace = var.project_id
  }

  spec {
    route_name = google_cloud_run_v2_service.dashboard.name
  }

  depends_on = [google_cloud_run_v2_service.dashboard]
}
