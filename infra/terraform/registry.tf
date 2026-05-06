# ---------------------------------------------------------------------------
# Artifact Registry — Docker repository for gateway / ingress / cli images.
# ---------------------------------------------------------------------------

resource "google_artifact_registry_repository" "lynxx" {
  location      = var.region
  repository_id = "lynxx"
  description   = "Lynxx gateway container images"
  format        = "DOCKER"

  cleanup_policies {
    id     = "keep-last-10"
    action = "KEEP"
    most_recent_versions {
      keep_count = 10
    }
  }

  cleanup_policies {
    id     = "delete-untagged-after-7d"
    action = "DELETE"
    condition {
      tag_state    = "UNTAGGED"
      older_than   = "604800s"   # 7 days
    }
  }

  depends_on = [google_project_service.apis]
}

locals {
  registry_base = "${var.region}-docker.pkg.dev/${var.project_id}/lynxx"
}
