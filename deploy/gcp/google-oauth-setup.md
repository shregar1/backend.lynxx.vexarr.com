# Sign-in with Google — GCP setup

The OAuth Client ID for "Sign in with Google" cannot be provisioned
via `gcloud` for an *external* OAuth brand — Cloud Console is the
only supported path. The steps below set everything up in the
`vexarr-com-495315` project; once done, hand the Client ID to the
billing service and the website via env vars.

## 1. Configure the consent screen (one-time)

The brand `Vexarr` already exists (see `gcloud alpha iap oauth-brands
list`). Make sure it's set to **External** so customers from any
Google account can sign in:

- Visit <https://console.cloud.google.com/apis/credentials/consent?project=vexarr-com-495315>
- User type: **External**
- App name: `Lynxx`
- User support email: `support@lynxx.dev`
- Developer contact: your email
- Authorized domains: `lynxx.vexarr.com`, `vexarr.com`
- Scopes: leave at the defaults (`openid`, `email`, `profile`)

## 2. Create the OAuth Client ID

- Visit <https://console.cloud.google.com/apis/credentials?project=vexarr-com-495315>
- **+ CREATE CREDENTIALS** → **OAuth client ID**
- Application type: **Web application**
- Name: `Lynxx Web`
- Authorized JavaScript origins:
  - `https://lynxx.vexarr.com`
  - `https://www.lynxx.vexarr.com`
  - `https://lynxx-website-588755157227.us-central1.run.app`
  - `https://lynxx-website-7jnb4axmhq-uc.a.run.app`
  - `http://localhost:5173` (dev)
- Authorized redirect URIs:
  - `https://lynxx.vexarr.com`
  - `https://lynxx-website-588755157227.us-central1.run.app`
  - `http://localhost:5173`
- Click **Create** and copy the Client ID (looks like
  `123456789-abc.apps.googleusercontent.com`).

## 3. Store the Client ID

```bash
# Backend reads it from this secret at startup.
echo -n "<your-client-id>.apps.googleusercontent.com" | \
  gcloud secrets create LYNXX_GOOGLE_OAUTH_CLIENT_ID \
  --project=vexarr-com-495315 --data-file=- --labels=product=lynxx

# Same value is baked into the website bundle at build time.
# Pass it to `docker buildx build` via:
#   --build-arg VITE_GOOGLE_OAUTH_CLIENT_ID=<the-id>
```

## 4. Wire it into Cloud Run

Terraform (`infra/terraform/cloudrun.tf`) injects
`GOOGLE_OAUTH_CLIENT_ID` from the secret. Apply once:

```bash
cd infra/terraform
terraform apply -auto-approve -target=google_cloud_run_v2_service.billing
```

For the website, rebuild the image with the build-arg:

```bash
docker buildx build --platform=linux/amd64 \
  --build-arg VITE_BILLING_API_URL=https://api.lynxx.vexarr.com \
  --build-arg VITE_GOOGLE_OAUTH_CLIENT_ID="$(gcloud secrets versions access latest --secret=LYNXX_GOOGLE_OAUTH_CLIENT_ID)" \
  -t us-central1-docker.pkg.dev/vexarr-com-495315/lynxx/lynxx-website:latest \
  --push .
gcloud run services update lynxx-website --region=us-central1 \
  --image=us-central1-docker.pkg.dev/vexarr-com-495315/lynxx/lynxx-website:latest \
  --port=80
```

## How the flow works

1. The website loads Google Identity Services (`accounts.google.com/gsi/client`).
2. The Google button mounts; on click, Google returns an **ID token**
   (a signed JWT) to the website's JS callback.
3. Website POSTs `{ id_token }` to `/v1/auth/google` with
   `credentials: include` and the `X-Lynxx-Web` header.
4. Backend verifies the JWT against Google's JWKS
   (`https://www.googleapis.com/oauth2/v3/certs`), checks
   `iss == accounts.google.com`, `aud == GOOGLE_OAUTH_CLIENT_ID`, and
   `exp > now`.
5. Backend looks up the customer by email:
   - **Existing customer** → issues a new session, sets the cookie.
   - **New customer** → inserts a row with a random argon2 password
     hash (32 bytes hex, never used — Google is the only auth path
     for this customer until they reset the password) plus a default
     "Free" tier API key, then issues the session.
6. Cookie flows back to the website; subsequent requests look the
   same as email/password sign-in.
