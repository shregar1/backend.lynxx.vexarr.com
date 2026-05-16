# keyx licence setup

How to point lynxx-gateway at a keyx-issued licence bundle instead of
the legacy billing-server flow.

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

## Differences from the legacy flow

| | Legacy (`LYNXX_API_KEY`) | keyx |
|---|---|---|
| Server | Vexarr billing | api.keyx.vexarr.com |
| Auth surface | `lnx_live_...` opaque key | URN + service token + signed bundle |
| Network calls per boot | 1 verify + 1 register | 1 verify (no register) |
| Node accounting | Yes — `register_node` + heartbeat + usage reporter | **No** — Phase 1 keyx doesn't ship those; per-tenant quotas live in the signed payload |
| Cache | `~/.lynxx/license.cache.json`, 7-day grace | same path, same grace |
| Revocation latency | 6 h (re-verify interval) | 6 h (re-verify interval) |
| Tier / quota update | Live — picked up on next re-verify | Requires a new signed bundle + restart |
| `LYNXX_API_KEY` env | Required | Ignored |
| Sample `.env` | [`.env.example`](../.env.example) | [`.env.example`](../.env.example) |

## Troubleshooting

**`licence verification failed: keyx rejected the licence signature`**
The bundle doesn't round-trip against the URN. Either the bytes were
mangled in transit (whitespace, BOM, line-wrapping) or Vexarr revoked
the signing key. Confirm the `payload` + `signature` are exactly what
ops sent (a single base64 token each, no embedded newlines). If the
bytes are clean, contact ops to confirm the URN is still active.

**`licence verification failed: keyx not configured`**
At least one of `KEYX_BASE_URL`, `KEYX_SERVICE_TOKEN`,
`KEYX_LICENSE_URN`, `[license].payload`, `[license].signature` is
empty or unset. The gateway will fall back to legacy mode in this
case — set `LYNXX_API_KEY` to a valid `lnx_live_…` key, or finish
filling the keyx fields.

**`licence verification failed: keyx request failed: ...`**
Transport error reaching keyx. Check egress: lynxx needs outbound
HTTPS to `api.keyx.vexarr.com:443`. If a warm cache exists the
gateway will still start (logs say `from_cache=true`); past the
7-day grace it hard-fails.

**Tier / quota didn't update after I got a new licence**
Restart the gateway — the payload is parsed once at boot. The 6-hour
re-verifier only flips the kill switch on revocation; it doesn't
re-parse a swapped-in bundle.

## Reverting to legacy mode

Remove all four of these from your config / env:

```
KEYX_BASE_URL, KEYX_SERVICE_TOKEN, KEYX_LICENSE_URN
[license].payload, [license].signature
```

…and put `LYNXX_API_KEY=lnx_live_…` back. The gateway picks the
legacy code path on the next start. No data migration; the
`~/.lynxx/license.cache.json` from keyx mode is keyed by signature
and is simply ignored.
