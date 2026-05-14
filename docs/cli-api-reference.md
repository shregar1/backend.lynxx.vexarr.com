# Lynxx Gateway — CLI / API Reference

Every feature exposed by the admin dashboard is available over HTTP.
All endpoints live on the **admin port** (default `9901`).

```
BASE=http://localhost:9901
TOKEN=lynxx_xxxxxxxx
AUTH="-H 'Authorization: Bearer $TOKEN'"
```

> **Response envelope** — every JSON response is wrapped:
> ```json
> {
>   "transactionUrn": "urn:lynxx:txn:<uuid>",
>   "status": "SUCCESS" | "FAILED",
>   "responseMessage": "...",
>   "responseKey": "SUCCESS | NOT_FOUND | VALIDATION_ERROR | …",
>   "errors": [],
>   "timestamp": "2026-05-14T12:00:00Z",
>   "metadata": {},
>   "data": <actual payload>,
>   "reference_urn": ""
> }
> ```
> Use `jq .data` to extract the payload in scripts.

---

## Authentication & Tokens

Roles: `viewer` (read-only), `operator` (mutations), `admin` (token management).

| Action | Command |
|--------|---------|
| **List tokens** | `curl -s $BASE/tokens $AUTH \| jq .data` |
| **Create token** | `curl -s -X POST $BASE/tokens $AUTH -H 'Content-Type: application/json' -d '{"name":"ci","role":"operator"}'` |
| **Update token** | `curl -s -X PUT $BASE/tokens/:id $AUTH -H 'Content-Type: application/json' -d '{"name":"ci-renamed","role":"admin"}'` |
| **Delete token** | `curl -s -X DELETE $BASE/tokens/:id $AUTH` |

The raw token string is returned **once** on creation in `data.token`. Store it securely.

```bash
# Create an operator token and save it
curl -s -X POST $BASE/tokens $AUTH \
  -H 'Content-Type: application/json' \
  -d '{"name":"deploy-bot","role":"operator"}' | jq -r '.data.token'
```

---

## Projects

| Action | Command |
|--------|---------|
| **List** | `curl -s $BASE/projects $AUTH` |
| **Get** | `curl -s $BASE/projects/:id $AUTH` |
| **Create** | `curl -s -X POST $BASE/projects $AUTH -H 'Content-Type: application/json' -d '{"name":"my-app","description":"Production app"}'` |
| **Update** | `curl -s -X PATCH $BASE/projects/:id $AUTH -H 'Content-Type: application/json' -d '{"name":"renamed"}'` |
| **Delete** | `curl -s -X DELETE $BASE/projects/:id $AUTH` |
| **List project services** | `curl -s $BASE/projects/:id/services $AUTH` |

Deleting a project cascade-soft-deletes all its services, routes, targets, and plugins.

---

## Services

| Action | Command |
|--------|---------|
| **List** | `curl -s $BASE/services $AUTH` |
| **Get** | `curl -s $BASE/services/:id $AUTH` |
| **Create** | see below |
| **Update (full)** | `curl -s -X PUT $BASE/services/:id $AUTH -H 'Content-Type: application/json' -d '{...}'` |
| **Patch** | `curl -s -X PATCH $BASE/services/:id $AUTH -H 'Content-Type: application/json' -d '{"retries":5}'` |
| **Delete** | `curl -s -X DELETE $BASE/services/:id $AUTH` |

```bash
# Create a service with subdomain routing
curl -s -X POST $BASE/services $AUTH \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "users-api",
    "url": "http://users-svc:3000",
    "retries": 3,
    "connect_timeout_ms": 5000,
    "lb_strategy": "round-robin",
    "project_id": "<project-uuid>",
    "domain": "example.com",
    "subdomain": "api"
  }'
```

Service fields:

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `name` | string | **required** | Unique name |
| `url` | string | `""` | Upstream base URL |
| `retries` | int | `3` | Retry count on failure |
| `connect_timeout_ms` | int | `5000` | Upstream connect timeout |
| `lb_strategy` | string | `"round-robin"` | `round-robin`, `random`, `ip-hash`, `least-conn`, `header-hash`, `query-hash` |
| `service_type` | string | `"api"` | `api` or `static` |
| `document_root` | string | null | For `static` services |
| `index_file` | string | `"index.html"` | For `static` services |
| `spa_fallback` | bool | `false` | Serve index.html for 404s |
| `project_id` | uuid | null | Parent project |
| `domain` | string | `""` | Root domain for subdomain routing |
| `subdomain` | string | `""` | Subdomain prefix |

---

## Targets (Upstreams)

Each service has one or more weighted targets for load balancing.

| Action | Command |
|--------|---------|
| **List all** | `curl -s $BASE/targets $AUTH` |
| **List for service** | `curl -s $BASE/services/:id/targets $AUTH` |
| **Get** | `curl -s $BASE/targets/:id $AUTH` |
| **Create** | `curl -s -X POST $BASE/services/:id/targets $AUTH -H 'Content-Type: application/json' -d '{"url":"http://backend-2:3000","weight":50}'` |
| **Update** | `curl -s -X PUT $BASE/targets/:id $AUTH -H 'Content-Type: application/json' -d '{"url":"http://backend-2:3001","weight":100,"enabled":true}'` |
| **Delete** | `curl -s -X DELETE $BASE/targets/:id $AUTH` |

Target fields: `url` (required), `weight` (default 1), `enabled` (default true), `zone` (optional).

---

## Routes

| Action | Command |
|--------|---------|
| **List** | `curl -s $BASE/routes $AUTH` |
| **Get** | `curl -s $BASE/routes/:id $AUTH` |
| **Create** | see below |
| **Update (full)** | `curl -s -X PUT $BASE/routes/:id $AUTH -H 'Content-Type: application/json' -d '{...}'` |
| **Patch** | `curl -s -X PATCH $BASE/routes/:id $AUTH -H 'Content-Type: application/json' -d '{"strip_path":false}'` |
| **Delete** | `curl -s -X DELETE $BASE/routes/:id $AUTH` |
| **Test** | see below |

```bash
# Create a route
curl -s -X POST $BASE/routes $AUTH \
  -H 'Content-Type: application/json' \
  -d '{
    "paths": ["/api/v1/users", "/api/v1/users/*"],
    "methods": ["GET", "POST"],
    "hosts": ["api.example.com"],
    "service_id": "<service-uuid>",
    "strip_path": true
  }'
```

### Route Testing

Fire a test request through the gateway (external) and directly to the upstream (internal), then compare results.

```bash
curl -s -X POST $BASE/routes/:id/test $AUTH \
  -H 'Content-Type: application/json' \
  -d '{
    "method": "GET",
    "path": "/api/v1/users",
    "headers": {"X-Custom": "value"}
  }'
```

Response:
```json
{
  "data": {
    "external": {
      "url": "http://127.0.0.1:8080/api/v1/users",
      "status": 200,
      "latency_ms": 42,
      "headers": {"content-type": "application/json"},
      "body_preview": "{\"users\":[...]}",
      "healthy": true
    },
    "internal": {
      "url": "http://users-svc:3000/api/v1/users",
      "status": 200,
      "latency_ms": 12,
      "headers": {"content-type": "application/json"},
      "body_preview": "{\"users\":[...]}",
      "healthy": true
    }
  }
}
```

A status mismatch between external and internal indicates gateway-level modification (plugins, rate limiting, auth).

---

## Plugins

| Action | Command |
|--------|---------|
| **List** | `curl -s $BASE/plugins $AUTH` |
| **Get** | `curl -s $BASE/plugins/:id $AUTH` |
| **Create** | see below |
| **Update** | `curl -s -X PUT $BASE/plugins/:id $AUTH -H 'Content-Type: application/json' -d '{...}'` |
| **Delete** | `curl -s -X DELETE $BASE/plugins/:id $AUTH` |

```bash
# Create a rate-limiter scoped to a service
curl -s -X POST $BASE/plugins $AUTH \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "rate-limiter",
    "service_id": "<service-uuid>",
    "enabled": true,
    "config": {"requests_per_second": 100, "burst": 20}
  }'
```

Plugin fields: `name` (required), `service_id` (optional scope), `route_id` (optional scope), `enabled` (default true), `config` (JSON object).

---

## Consumers

| Action | Command |
|--------|---------|
| **List** | `curl -s $BASE/consumers $AUTH` |
| **Create** | `curl -s -X POST $BASE/consumers $AUTH -H 'Content-Type: application/json' -d '{"username":"app-client","custom_id":"tenant-42"}'` |
| **Delete** | `curl -s -X DELETE $BASE/consumers/:id $AUTH` |

---

## Health & Status

| Action | Command |
|--------|---------|
| **Health check (liveness)** | `curl -s $BASE/healthz` |
| **All service health** | `curl -s $BASE/health $AUTH` |
| **Single service health** | `curl -s $BASE/health/:service_id $AUTH` |
| **Gateway status** | `curl -s $BASE/status $AUTH` |
| **Upstream status** | `curl -s $BASE/upstreams $AUTH` |
| **Prometheus metrics** | `curl -s $BASE/metrics $AUTH` |
| **Quota snapshot** | `curl -s $BASE/quota $AUTH` |

```bash
# Quick health probe for scripts
curl -sf $BASE/healthz > /dev/null && echo "UP" || echo "DOWN"

# Prometheus scrape target
# Add to prometheus.yml:
#   - targets: ['gateway:9901']
#     metrics_path: '/metrics'
```

---

## Circuit Breakers

| Action | Command |
|--------|---------|
| **List all** | `curl -s $BASE/circuit-breakers $AUTH` |
| **Reset for service** | `curl -s -X POST $BASE/circuit-breakers/:service_id/reset $AUTH` |

---

## Request Logs

Logs are buffered and flushed every 2 seconds. Pruned after 30 days.

| Action | Command |
|--------|---------|
| **List** | `curl -s "$BASE/logs?limit=50" $AUTH` |
| **Filter by service** | `curl -s "$BASE/logs?service_id=<uuid>" $AUTH` |
| **Filter by level** | `curl -s "$BASE/logs?level=error" $AUTH` |
| **Search** | `curl -s "$BASE/logs?search=timeout" $AUTH` |

Query parameters: `service_id`, `route_id`, `project_id`, `level`, `search`, `limit` (default 200, max 1000).

```bash
# Tail errors for a specific service
watch -n5 'curl -s "$BASE/logs?service_id=<uuid>&level=error&limit=10" $AUTH | jq ".data[] | {ts,message,status,path}"'
```

---

## Audit Log

| Action | Command |
|--------|---------|
| **List** | `curl -s "$BASE/audit?limit=50" $AUTH` |
| **Verify chain integrity** | `curl -s $BASE/audit/verify $AUTH` |

Query parameters: `actor`, `action`, `resource_type`, `since` (ISO 8601), `limit`, `include_diff` (bool).

```bash
# Show recent deletions
curl -s "$BASE/audit?action=delete&limit=20" $AUTH | jq '.data[] | {ts,action,resource_type,resource_id}'
```

---

## Recordings (Time-Travel Debug)

| Action | Command |
|--------|---------|
| **List** | `curl -s "$BASE/recordings?limit=20" $AUTH` |
| **Get** | `curl -s $BASE/recordings/:id $AUTH` |
| **Delete** | `curl -s -X DELETE $BASE/recordings/:id $AUTH` |
| **Replay** | `curl -s -X POST $BASE/recordings/:id/replay $AUTH -H 'Content-Type: application/json' -d '{}'` |

Query parameters for list: `since`, `method`, `path_prefix`, `status`, `service_id`, `limit`.

```bash
# Replay a recorded request through the gateway
curl -s -X POST $BASE/recordings/<sha256>/replay $AUTH \
  -H 'Content-Type: application/json' \
  -d '{"target_url": "http://127.0.0.1:8080"}'

# Replay directly to an upstream (bypassing gateway)
curl -s -X POST $BASE/recordings/<sha256>/replay $AUTH \
  -H 'Content-Type: application/json' \
  -d '{"target_url": "http://users-svc:3000"}'
```

---

## Zero-Downtime Deploy

| Action | Command |
|--------|---------|
| **Start deploy** | see below |
| **Check status** | `curl -s $BASE/services/:id/deploy/status $AUTH` |

```bash
# Deploy a new version with health gating
curl -s -X POST $BASE/services/:id/deploy $AUTH \
  -H 'Content-Type: application/json' \
  -d '{
    "new_target_url": "http://backend-v2:3000",
    "health_check_path": "/health",
    "health_check_interval_ms": 2000,
    "health_check_timeout_ms": 3000,
    "health_check_threshold": 3,
    "drain_timeout_ms": 30000,
    "weight": 100,
    "webhook_url": "https://hooks.slack.com/...",
    "webhook_secret": "secret"
  }'
```

Flow: add new target → health-check until threshold → drain old targets → done.

---

## ACME / TLS Certificates

| Action | Command |
|--------|---------|
| **Status** | `curl -s $BASE/acme/status $AUTH` |
| **Renew** | `curl -s -X POST "$BASE/acme/renew?force=true" $AUTH` |
| **Revoke** | `curl -s -X POST "$BASE/acme/revoke?reason=superseded" $AUTH` |
| **Set HTTP-01 challenge** | `curl -s -X POST $BASE/acme/http-01-challenges $AUTH -H 'Content-Type: application/json' -d '{"token":"...","key_authorization":"..."}'` |
| **Clear HTTP-01 challenge** | `curl -s -X DELETE $BASE/acme/http-01-challenges/:token $AUTH` |
| **Set TLS-ALPN-01 challenge** | `curl -s -X POST $BASE/acme/tls-alpn-01-challenges $AUTH -H 'Content-Type: application/json' -d '{"domain":"example.com","key_authorization":"..."}'` |
| **Clear TLS-ALPN-01 challenge** | `curl -s -X DELETE $BASE/acme/tls-alpn-01-challenges/:domain $AUTH` |

Revocation reasons: `key_compromise`, `affiliation_changed`, `superseded`, `cessation_of_operation`.

---

## Live Config

Base path: `/admin/v1/config`

| Action | Command |
|--------|---------|
| **Get current** | `curl -s $BASE/admin/v1/config $AUTH` |
| **Replace entire config** | `curl -s -X PUT $BASE/admin/v1/config $AUTH -H 'Content-Type: application/json' -d @config.json` |
| **Patch a key** | `curl -s -X PATCH "$BASE/admin/v1/config/gateway.proxy_port" $AUTH -H 'Content-Type: application/json' -d '{"value": 8081}'` |
| **Validate** | `curl -s -X POST $BASE/admin/v1/config/validate $AUTH -H 'Content-Type: application/json' -d @config.json` |
| **History** | `curl -s $BASE/admin/v1/config/history $AUTH` |
| **Lookup by ID tag** | `curl -s "$BASE/admin/v1/config/@id/:tag" $AUTH` |

```bash
# Change the proxy port without a restart
curl -s -X PATCH "$BASE/admin/v1/config/gateway.proxy_port" $AUTH \
  -H 'Content-Type: application/json' \
  -d '{"value": 8081}'
```

---

## Debug Profiling

| Action | Command |
|--------|---------|
| **Heap** | `curl -s $BASE/debug/pprof/heap $AUTH` |
| **Tokio tasks** | `curl -s $BASE/debug/pprof/tasks $AUTH` |
| **Threads** | `curl -s $BASE/debug/pprof/threads $AUTH` |
| **Uptime / runtime** | `curl -s $BASE/debug/pprof/uptime $AUTH` |
| **Combined stats** | `curl -s $BASE/debug/pprof/stats $AUTH` |

---

## Reload

Force a full config reload from the database.

```bash
curl -s -X POST $BASE/reload $AUTH
```

---

## ETag Concurrency Control

`GET` responses for individual resources include an `ETag` header. Pass `If-Match` on mutations to prevent stale writes:

```bash
# Fetch the service and capture its ETag
ETAG=$(curl -sI $BASE/services/:id $AUTH | grep -i etag | awk '{print $2}' | tr -d '\r')

# Update with optimistic concurrency
curl -s -X PATCH $BASE/services/:id $AUTH \
  -H 'Content-Type: application/json' \
  -H "If-Match: $ETAG" \
  -d '{"retries": 5}'
```

If the resource changed since you read it, you get `412 Precondition Failed`.

---

## Error Codes

The `responseKey` field in the envelope maps to:

| HTTP Status | responseKey | Meaning |
|-------------|-------------|---------|
| 400 | `VALIDATION_ERROR` | Bad input |
| 401 | `UNAUTHORIZED` | Missing or invalid token |
| 403 | `FORBIDDEN` | Insufficient role |
| 404 | `NOT_FOUND` | Resource doesn't exist |
| 409 | `CONFLICT` | Duplicate / unique constraint |
| 412 | `PRECONDITION_FAILED` | ETag mismatch |
| 429 | `QUOTA_EXCEEDED` | Monthly quota hit |
| 5xx | `INTERNAL_ERROR` | Server-side failure |

---

## Shell Helpers

```bash
# lynxx() — shorthand for authenticated admin calls
lynxx() {
  curl -s "http://localhost:9901/$1" \
    -H "Authorization: Bearer $LYNXX_TOKEN" \
    "${@:2}" | jq .data
}

# Usage:
lynxx services
lynxx services -X POST -H 'Content-Type: application/json' -d '{"name":"foo","url":"http://localhost:3000"}'
lynxx routes/:id/test -X POST -H 'Content-Type: application/json' -d '{"method":"GET","path":"/"}'
```
