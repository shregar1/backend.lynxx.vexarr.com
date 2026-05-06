output "gateway_ip" {
  description = "Global IP for the API gateway load balancer. Point your DNS A record here."
  value       = google_compute_global_address.gateway.address
}

output "gke_cluster_name" {
  description = "GKE cluster name."
  value       = google_container_cluster.lynxx.name
}

output "gke_connect_cmd" {
  description = "gcloud command to configure kubectl."
  value       = "gcloud container clusters get-credentials ${google_container_cluster.lynxx.name} --region ${var.region} --project ${var.project_id}"
}

output "registry_base" {
  description = "Artifact Registry base URL — prefix all image names with this."
  value       = local.registry_base
}

output "sql_connection_name" {
  description = "Cloud SQL instance connection name — used by Cloud SQL Auth Proxy."
  value       = google_sql_database_instance.lynxx.connection_name
}

output "sql_private_ip" {
  description = "Cloud SQL private IP (for direct connections within the VPC)."
  value       = google_sql_database_instance.lynxx.private_ip_address
}

output "website_url" {
  description = "Cloud Run URL for the marketing website."
  value       = google_cloud_run_v2_service.website.uri
}

output "dashboard_url" {
  description = "Cloud Run URL for the customer dashboard."
  value       = google_cloud_run_v2_service.dashboard.uri
}

output "billing_url" {
  description = "Cloud Run URL for the licence/billing service. Set this as VITE_BILLING_API_URL on the website (or point api.lynxx.dev at it via DNS)."
  value       = google_cloud_run_v2_service.billing.uri
}

output "wif_provider" {
  description = "Workload Identity Federation provider resource name — set as GitHub variable WIF_PROVIDER."
  value       = google_iam_workload_identity_pool_provider.github.name
}

output "cicd_sa_email" {
  description = "CI/CD service account email — set as GitHub variable WIF_SERVICE_ACCOUNT."
  value       = google_service_account.cicd.email
}

output "workload_identity_annotation" {
  description = "K8s ServiceAccount annotation needed for the gateway pods."
  value       = "iam.gke.io/gcp-service-account: ${google_service_account.gateway.email}"
}
