# GCP Deployment Guide

Complete bootstrap from zero to a running production cluster.

---

## Prerequisites

| Tool | Min version |
|------|-------------|
| `gcloud` CLI | 470+ |
| `terraform` | 1.7+ |
| `helm` | 3.14+ |
| `kubectl` | 1.29+ |
| `gh` CLI | 2.40+ |

```bash
gcloud auth login
gcloud auth application-default login
```

---

## Step 1 — GCS bucket for Terraform state

Create this once. It must exist before `terraform init`.

```bash
PROJECT_ID="your-gcp-project-id"
BUCKET="your-project-tfstate"    # globally unique

gsutil mb -p "$PROJECT_ID" -l us-central1 "gs://$BUCKET"
gsutil versioning set on "gs://$BUCKET"
```

---

## Step 2 — Terraform variables

```bash
cd infra/terraform
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars     # fill in project_id, bucket, domains, db_password
```

Key values:
- `project_id` — from `gcloud projects list`
- `tf_state_bucket` — bucket name from Step 1
- `domain` — e.g. `api.lynxx.dev` (the gateway endpoint)
- `db_password` — pick a strong password (≥32 chars)

---

## Step 3 — Terraform init / plan / apply

```bash
terraform init \
  -backend-config="bucket=$(grep tf_state_bucket terraform.tfvars | cut -d'"' -f2)" \
  -backend-config="prefix=lynxx/prod"

terraform plan -out=tfplan
terraform apply tfplan
```

First apply takes ~15 minutes (GKE Autopilot cluster provisioning).

---

## Step 4 — Capture outputs

```bash
terraform output -json > /tmp/tf-outputs.json

GATEWAY_IP=$(terraform output -raw gateway_ip)
SQL_CONN=$(terraform output -raw sql_connection_name)
REGISTRY=$(terraform output -raw registry_base)
CICD_KEY=$(terraform output -raw cicd_sa_key_base64)

echo "Gateway IP : $GATEWAY_IP"
echo "SQL conn   : $SQL_CONN"
echo "Registry   : $REGISTRY"
```

---

## Step 5 — DNS records

Cloud Run domain mappings for `api.`, `lynxx.vexarr.com`, and
`app.lynxx.vexarr.com` populate the records that need to be added at
the authoritative DNS provider for `lynxx.vexarr.com`. Pull them via:

```bash
terraform output -json billing_domain_dns
terraform output -json website_domain_dns
terraform output -json dashboard_domain_dns
```

Each entry is `{ name, type, rrdata }` — add the matching record set
(typically a CNAME pointing at `ghs.googlehosted.com.` for subdomains).
The gateway LB stays on its own A record:

| Record | Type | Value |
|--------|------|-------|
| `api.lynxx.vexarr.com` | A/AAAA | from `billing_domain_dns` |
| `lynxx.vexarr.com` | A/AAAA | from `website_domain_dns` |
| `app.lynxx.vexarr.com` | A/AAAA | from `dashboard_domain_dns` |

TLS certificates are Google-managed and provision automatically once
DNS resolves (up to 24 h for first issuance).

---

## Step 6 — GitHub repository secrets & variables

Run these in all three repos (`backend.lynxx.dev`, `website.lynxx.dev`, `node.dashboard.lynxx.dev`):

```bash
REPO_BACKEND="$(gh repo view --json nameWithOwner -q .nameWithOwner)"

# Secrets
gh secret set GCP_SA_KEY          --body "$CICD_KEY"           -R "$REPO_BACKEND"
gh secret set DB_PASSWORD         --body "$(grep db_password terraform.tfvars | cut -d'"' -f2)" \
                                                                -R "$REPO_BACKEND"

# Variables
gh variable set GCP_PROJECT_ID    --body "$PROJECT_ID"         -R "$REPO_BACKEND"
gh variable set GCP_REGION        --body "us-central1"         -R "$REPO_BACKEND"
gh variable set GKE_CLUSTER_NAME  --body "lynxx-cluster"       -R "$REPO_BACKEND"
gh variable set SQL_CONNECTION_NAME --body "$SQL_CONN"         -R "$REPO_BACKEND"
```

For the website repo (Stripe keys — get from Stripe dashboard):

```bash
REPO_WEB="$(gh repo view --json nameWithOwner -q .nameWithOwner -R org/website.lynxx.dev)"

gh secret set VITE_STRIPE_PUBLISHABLE_KEY   --body "pk_live_..."  -R "$REPO_WEB"
gh secret set VITE_STRIPE_PRO_PRICE_ID      --body "price_..."    -R "$REPO_WEB"
gh secret set VITE_STRIPE_BUSINESS_PRICE_ID --body "price_..."    -R "$REPO_WEB"
gh variable set VITE_BILLING_API_URL        --body "/api/billing"  -R "$REPO_WEB"
```

For the dashboard repo:

```bash
REPO_DASH="$(gh repo view --json nameWithOwner -q .nameWithOwner -R org/node.dashboard.lynxx.dev)"

gh variable set VITE_GATEWAY_API_URL   --body "https://api.lynxx.dev"  -R "$REPO_DASH"
gh variable set VITE_GATEWAY_ADMIN_URL --body "https://api.lynxx.dev/admin" -R "$REPO_DASH"
```

Cross-repo PAT (needed for the deploy workflow to check out website + dashboard):

```bash
# Create a fine-grained PAT with "Contents: read" on both frontend repos, then:
gh secret set GH_PAT --body "github_pat_..." -R "$REPO_BACKEND"
```

---

## Step 7 — First deploy

Push to `main` (or trigger manually):

```bash
git push origin main
# or
gh workflow run deploy-gcp.yml --field environment=prod -R "$REPO_BACKEND"
```

Watch the run:

```bash
gh run watch -R "$REPO_BACKEND"
```

---

## Step 8 — Verify

```bash
# Get kubectl access
$(terraform output -raw gke_connect_cmd)

# Check pods
kubectl get pods -n lynxx-system

# Hit the gateway
curl -si https://api.lynxx.dev/status

# Cloud Run health
curl -si "$(terraform output -raw website_url)/healthz"
curl -si "$(terraform output -raw dashboard_url)/healthz"
```

---

## Secrets that must be set manually post-deploy

These are created as empty Secret Manager secrets by Terraform and must be filled in:

```bash
# Stripe secret key
echo -n "sk_live_..." | gcloud secrets versions add lynxx-stripe-secret-key \
  --project "$PROJECT_ID" --data-file=-

# Stripe webhook signing secret
echo -n "whsec_..." | gcloud secrets versions add lynxx-stripe-webhook-secret \
  --project "$PROJECT_ID" --data-file=-
```

---

## Staging environment

```bash
# Re-init with a different prefix and apply with staging overrides
terraform init \
  -backend-config="bucket=$BUCKET" \
  -backend-config="prefix=lynxx/staging" \
  -reconfigure

terraform apply \
  -var="environment=staging" \
  -var="db_tier=db-f1-micro" \
  -var="gateway_replicas=1" \
  -var="cluster_name=lynxx-staging"
```

---

## Destroy

```bash
# Removes everything except the TF state bucket.
# Cloud SQL has deletion_protection=true — disable it first:
terraform apply -var="environment=prod" \
  -target=google_sql_database_instance.lynxx \
  # (edit sql.tf: deletion_protection = false, then apply, then destroy)

terraform destroy
```
