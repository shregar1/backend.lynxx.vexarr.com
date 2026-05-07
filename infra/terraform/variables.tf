variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "region" {
  description = "Primary GCP region."
  type        = string
  default     = "us-central1"
}

variable "zone" {
  description = "Primary GCP zone (used for Cloud SQL read replica placement)."
  type        = string
  default     = "us-central1-a"
}

variable "environment" {
  description = "Deployment environment label (prod / staging)."
  type        = string
  default     = "prod"
}

variable "cluster_name" {
  description = "GKE cluster name."
  type        = string
  default     = "lynxx-cluster"
}

# Cloud SQL is shared across Vexarr products — see infra/terraform/sql.tf.
# The lynxx-specific DB user, database, and password (LYNXX_DB_PASSWORD)
# are managed out-of-band via gcloud, not Terraform. No db_* variables
# are declared here.

variable "domain" {
  description = "Primary domain for the gateway load balancer (e.g. api.lynxx.dev)."
  type        = string
}

variable "website_domain" {
  description = "Marketing website domain (e.g. lynxx.vexarr.com)."
  type        = string
}

variable "dashboard_domain" {
  description = "Dashboard domain (e.g. app.lynxx.vexarr.com)."
  type        = string
}

variable "tf_state_bucket" {
  description = "GCS bucket name for Terraform remote state. Created manually before first apply."
  type        = string
}

variable "gateway_replicas" {
  description = "Number of gateway pod replicas."
  type        = number
  default     = 2
}

variable "plan_tier" {
  description = "Lynxx licence tier (free | pro | business | enterprise). Baked into gateway config."
  type        = string
  default     = "pro"
}

variable "stripe_price_pro" {
  description = "Stripe Price ID for the Pro tier subscription. Set after creating products in the Stripe dashboard."
  type        = string
  default     = ""
}

variable "stripe_price_business" {
  description = "Stripe Price ID for the Business tier subscription."
  type        = string
  default     = ""
}
