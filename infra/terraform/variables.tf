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

variable "db_instance_name" {
  description = "Cloud SQL instance name."
  type        = string
  default     = "lynxx-postgres"
}

variable "db_tier" {
  description = "Cloud SQL machine tier."
  type        = string
  default     = "db-perf-optimized-N-2"   # 2 vCPU, 16 GB — change to db-f1-micro for staging
}

variable "db_password" {
  description = "Postgres password for the lynxx user. Set via TF_VAR_db_password."
  type        = string
  sensitive   = true
}

variable "domain" {
  description = "Primary domain for the gateway load balancer (e.g. api.lynxx.dev)."
  type        = string
}

variable "website_domain" {
  description = "Marketing website domain (e.g. lynxx.dev)."
  type        = string
}

variable "dashboard_domain" {
  description = "Dashboard domain (e.g. app.lynxx.dev)."
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
