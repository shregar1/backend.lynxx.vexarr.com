# ---------------------------------------------------------------------------
# Cloud SQL — Postgres 16, private IP, automated backups, HA.
# ---------------------------------------------------------------------------

resource "random_id" "db_suffix" {
  byte_length = 4
}

resource "google_sql_database_instance" "lynxx" {
  name             = "${var.db_instance_name}-${random_id.db_suffix.hex}"
  database_version = "POSTGRES_16"
  region           = var.region

  deletion_protection = true   # prevents accidental terraform destroy

  settings {
    tier              = var.db_tier
    availability_type = "REGIONAL"   # HA — automatic failover to secondary zone

    backup_configuration {
      enabled                        = true
      point_in_time_recovery_enabled = true
      start_time                     = "03:00"   # 03:00 UTC
      transaction_log_retention_days = 7
      backup_retention_settings {
        retained_backups = 14
        retention_unit   = "COUNT"
      }
    }

    maintenance_window {
      day          = 7   # Sunday
      hour         = 4
      update_track = "stable"
    }

    ip_configuration {
      ipv4_enabled    = false   # no public IP
      private_network = google_compute_network.lynxx.id
    }

    database_flags {
      name  = "max_connections"
      value = "200"
    }

    insights_config {
      query_insights_enabled  = true
      record_application_tags = true
      record_client_address   = false
    }
  }

  depends_on = [google_service_networking_connection.sql]
}

resource "google_sql_database" "lynxx" {
  name     = "lynxx"
  instance = google_sql_database_instance.lynxx.name
}

resource "google_sql_user" "lynxx" {
  name     = "lynxx"
  instance = google_sql_database_instance.lynxx.name
  password = var.db_password
}
