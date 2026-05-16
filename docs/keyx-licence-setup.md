# keyx licence setup

Lynxx-gateway verifies its licence by asking keyx whether a signed
bundle (`payload` + `signature`) is still valid against an active
signing key. Without `[keyx]` configured, the gateway runs in
**unverified** mode — Enterprise tier, no kill-switch — which is
appropriate for self-host and dev deployments.

---

## What you need from Vexarr

When your contract starts, Vexarr ops will hand you three values:

```
license_urn  = 00000000-0000-0000-0000-000000000099
payload      = eyJjdXN0b21lcl91cm4iOiJjdXNfYWNtZSIsInRpZXIiOi...   (base64)
signature    = MEUCIQDvxR1NzZxQH7s7M2nE5fW...                       (base64)
```

Plus one shared piece of infra the same across all customers:

```
KEYX_SERVICE_TOKEN  = <bearer token>
```

(`base_url` defaults to `https://api.keyx.vexarr.com` — only set it
if you're self-hosting keyx, e.g. for an air-gapped install.)

## Drop it into `config.toml` + `.env`

```toml
# config.toml
[keyx]
base_url = "https://api.keyx.vexarr.com"   # optional override

[license]
payload   = "eyJjdXN0b21lcl91cm4iOi..."     # the base64 blob
signature = "MEUCIQDvxR1NzZx..."            # the base64 signature
```

```bash
# .env  (loaded by docker-compose; for systemd / bare-metal, set in unit file)
KEYX_SERVICE_TOKEN=<your bearer>
KEYX_LICENSE_URN=00000000-0000-0000-0000-000000000099
```

Both halves are required — the gateway only switches into keyx mode
when **all three** keyx env vars resolve AND both `[license].payload`
and `[license].signature` are present. Otherwise it falls back to the
legacy `LYNXX_API_KEY` flow, which keeps working unchanged.

## Verify the wiring

Restart the gateway and look for these log lines:

```
INFO  licence re-verifier in keyx mode
INFO  licence verified  customer_id=cus_acme  tier=pro  from_cache=false
```

`from_cache=false` confirms the gateway successfully reached keyx on
boot. After the first successful verify the bundle is cached at
`~/.lynxx/license.cache.json` (or `$LYNXX_CACHE_DIR/license.cache.json`
if you've set that). Subsequent restarts tolerate a keyx outage for
up to **7 days** before refusing to start.

## What the gateway does at runtime

1. **Boot** — POSTs `payload` + `signature` to
   `{base_url}/v1/keys/{license_urn}/verify-license`. Hard-fails the
   process if keyx says `valid: false` (revoked / tampered) and no
   warm cache exists.
2. **Every 6 hours** — re-checks via the same call. A confirmed
   `valid: false` flips the kill switch and the gateway starts
   returning HTTP 402 for every proxied request. Network failures
   are tolerated (the cache covers them).
3. **Tier / quota / expiry** — parsed locally from the signed
   payload, so the gateway doesn't have to hit keyx on the request
   path. To upgrade a customer, Vexarr issues a fresh
   `payload` + `signature` (URN stays the same) and the customer
   restarts the gateway — or hot-reloads config if their deployment
   supports it.

## Troubleshooting

**`licence verification failed: keyx rejected the licence signature`**
The bundle doesn't round-trip against the URN. Either the bytes were
mangled in transit (whitespace, BOM, line-wrapping) or Vexarr revoked
the signing key. Confirm the `payload` + `signature` are exactly what
ops sent (a single base64 token each, no embedded newlines). If the
bytes are clean, contact ops to confirm the URN is still active.

**`licence verification failed: [license].payload + [license].signature required when [keyx] is configured`**
You've configured `[keyx]` but haven't pasted the signed bundle.
Either fill `[license].payload` + `[license].signature` from the
`keyx license issue` output, or unset the `[keyx]` block to run in
unverified mode.

**`licence verification failed: keyx request failed: ...`**
Transport error reaching keyx. Check egress: lynxx needs outbound
HTTPS to `api.keyx.vexarr.com:443`. If a warm cache exists the
gateway will still start (logs say `from_cache=true`); past the
7-day grace it hard-fails.

**Tier / quota didn't update after I got a new licence**
Restart the gateway — the payload is parsed once at boot. The 6-hour
re-verifier only flips the kill switch on revocation; it doesn't
re-parse a swapped-in bundle.

## Bonus: keyx-issued admin tokens on the lynxx admin API

When `[keyx]` is configured, the admin-auth middleware accepts two
credential shapes on `Authorization: Bearer <token>`:

1. **Local `admin_tokens` table** (legacy) — `lnx_…` tokens created
   via `POST /admin/v1/tokens`. Hashed at rest, role enforced
   per-route. Unchanged.
2. **Keyx-issued tokens** (new) — any `ak_live_…` / `ak_test_…` key
   keyx returns a valid principal for via
   `POST /v1/keys/validate`. Role mapping:

   | keyx role | lynxx role |
   |---|---|
   | `admin`, `owner`        | `Admin`    |
   | `service`               | `Operator` |
   | `pat`, `viewer`         | `Viewer`   |
   | anything else           | rejected (401) |

The local lookup runs first; keyx is consulted only when the token
isn't in the local table. Failures (network, 401, unknown role) all
collapse to 401 — callers can't distinguish "unknown locally" from
"unknown to keyx" from "wrong tenant".

**Rotation / revocation:** by default every admin call hits keyx
live, so revoking a token in keyx takes effect on the next admin
request. Operators who want to absorb polling bursts (dashboards
hitting `/status` on a tick) can enable an in-memory cache:

```bash
# In .env or systemd unit
KEYX_ADMIN_CACHE_TTL_SECS=60
```

When set > 0, a successful keyx validate is cached by SHA-256 of the
raw token (raw tokens are never stored) for the TTL. Cap on
revocation staleness equals the TTL. Without keyx M4 (real-time
events) the TTL is the only knob; with M4 the cache can be longer
and busted on event.

**Disabling:** unset `KEYX_*` env vars (or leave `[keyx]`
unconfigured) and the fallback path is skipped — only local tokens
work.

**Per-route scope enforcement (optional):** keyx returns `scopes`
with every validated principal. The `require_scope("...")`
middleware layer enforces a specific scope on a route:

```rust
.route(
    "/admin/v1/services",
    post(create_service).layer(require_scope("services.write")),
)
```

Rules:
- `Role::Admin` actors satisfy every scope (master keys never need
  scope config)
- `"*"` in the actor's scope list satisfies any scope
- Bootstrap mode (no actor) satisfies any scope
- Legacy local-DB `admin_tokens` rows carry no scopes — non-Admin
  legacy tokens cannot satisfy scope checks. Migrate them to keyx
  for fine-grained access.

The library exposes `require_scope` and `actor_has_scope` but doesn't
yet apply them to any built-in admin route — operators wire it onto
routes they specifically want gated. A future increment will move
scope-required routes into a config-driven table.

## Bonus: route-level JWT verification via keyx JWKS

If your upstream services already accept keyx-signed JWTs (issued via
`POST /v1/keys/{urn}/issue-jwt`), the gateway's JWT plugin can verify
them at the edge by pointing at the same tenant's JWKS endpoint:

```toml
[[plugins]]
name = "jwt"
enabled = true

  [plugins.config]
  algorithm = "RS256"
  jwks_url  = "https://api.keyx.vexarr.com/v1/jwks.json"
```

Behaviour:
- The JWKS is fetched once on plugin load (or admin-reload) and cached
  for the plugin's lifetime
- Each request decodes the JWT header's `kid` and looks it up in the
  cached JWKS — automatic rotation as long as the old `kid` stays
  published during the overlap window
- Tokens missing a `kid`, or with a `kid` not in the JWKS, are rejected
- JWKS fetch failures are NOT silently fallthrough — the plugin
  refuses the request rather than letting an unverified token pass

Static `public_key` (PEM) and `secret` (HS\*) modes still work for
non-keyx setups.

## Bonus: HMAC verification via keyx

The `hmac-auth` plugin can route signature verification through keyx
instead of comparing against a locally-stored consumer secret:

```toml
[[plugins]]
name = "hmac-auth"
enabled = true

  [plugins.config]
  algorithms = ["hmac-sha256"]
  validate_request_body = true
  # Opt-in: which keyx signing key holds the HMAC secret
  keyx_urn = "00000000-0000-0000-0000-000000000099"
```

Two ways to wire the keyx client behind that URN:

1. **Shared (preferred)** — when `[keyx]` is configured globally, the
   plugin executor builds one `KeyxClient` at startup and threads it
   to every plugin via a `PluginContext`. Just set `keyx_urn` in the
   plugin config; the URL + service token come from the global block.
2. **Per-plugin (escape hatch)** — when you need a different keyx
   instance for a specific plugin, supply it inline:
   ```toml
   [plugins.config]
   keyx_urn           = "<uuid>"
   keyx_base_url      = "https://keyx-other.example/"
   keyx_service_token = "<bearer>"
   ```

**Behaviour:**
- Local consumer lookup is best-effort in keyx mode — the `username`
  in the Authorization header still resolves to a row in the
  `consumers` table (for the `x-consumer-username` / `x-consumer-id`
  downstream headers) but verification doesn't depend on it
- The canonicalised payload + provided signature are POSTed to
  `keyx /v1/keys/{urn}/verify-hmac`. Keyx's `verify_hmac` accepts
  **hex** signatures — the plugin handles the b64-to-hex conversion
- `valid: false` from keyx → 401 signature mismatch
- Transport / HTTP errors → 401 (no silent fallthrough)
- Without `keyx_urn`, the plugin behaves exactly as before (local
  consumer secret, computed in-process)

## Running without a licence

`[keyx]` is optional. When unset, `License::load` returns a
permissive default (Enterprise tier, no quotas, no kill-switch) and
the gateway runs without contacting any licence service. This is the
right mode for self-hosted / internal-only / dev deployments. The
admin `/status` endpoint reports `"licence_mode": "unverified"`.
