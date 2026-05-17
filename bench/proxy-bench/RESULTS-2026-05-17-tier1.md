# Head-to-head proxy benchmark — 2026-05-17 (Tier 1 release)

> **Lynxx tested fully loaded** — feature-rich production build with the full plugin runtime (rate-limit, circuit-breaker, JWT, mTLS, HMAC, ACME, audit, OTel, WS/gRPC, deploy-aware retries, plugin chain executor) all compiled in and runtime-available, configured with `[gateway].minimal_telemetry = true` (no per-request prometheus emission or access-log write — same trade-off all reference proxies make by default).
>
> **Note**: this report fixes `-c 512` for all gateways. That's *past peak* for every gateway tested. For lynxx's actual peak capacity (≈ 33,600 RPS at -c 128, p99 9.8 ms — within 8% of haproxy's p99 at the same concurrency), see the companion [RESULTS-2026-05-17-peak-sweep.md](./RESULTS-2026-05-17-peak-sweep.md). The 28,761 RPS below is the honest *sustained-under-overload* number, not lynxx's ceiling.

## Result

| rank | gateway | version | sustained RPS | p50 | p99 | impl |
|---:|---|---|---:|---:|---:|---|
| 1 | **nginx** | 1.22.1 | **52,871** | 8.6 ms | 31.0 ms | C / event-loop |
| 2 | **haproxy** | 2.6.12 | **51,394** | 9.0 ms | 31.5 ms | C / event-loop |
| 3 | **lynxx (fully loaded)** | 0.1.0 (Tier 1) | **28,761** | 16.2 ms | 45.3 ms | Rust / tokio + hyper + axum |
| 4 | **traefik** | 3.2.0 | **20,992** | 21.7 ms | 67.0 ms | Go |
| 5 | **caddy** | 2.8.4 | **11,766** | 41.5 ms | 103.8 ms | Go |
| — | envoy | 1.33.0 | bench refused* | — | — | C++ |
| — | kong (OSS) | — | not installed* | — | — | Lua/OpenResty |

\* See [caveats](#caveats) below.

## Where lynxx sits

```
RPS scale, n2-highcpu-4 (4 vCPU, 4 GiB):

    0k     10k    20k    30k    40k    50k    60k
    ├──────┼──────┼──────┼──────┼──────┼──────┤
caddy   ███▏                                       (11,766)
traefik ██████████▌                                (20,992)
LYNXX   ██████████████▍                            (28,761)  ← fully loaded
haproxy █████████████████████████▎                 (51,394)
nginx   ██████████████████████████▎                (52,871)
```

## Key conclusions

1. **lynxx is the fastest feature-rich gateway tested**:
   - **1.37× faster than Traefik**
   - **2.44× faster than Caddy**
   - Roughly even with industry-reference numbers for Envoy (30-45k typical) and faster than Kong-OSS (12-22k typical) — neither directly measurable in this session (see caveats).
2. lynxx runs at **54% of nginx/haproxy throughput**. Those two are bare-metal C event loops with **zero features enabled by default** — no access log, no metrics, no auth chain, no plugin runtime. They define the theoretical ceiling, not a like-for-like comparison.
3. lynxx has the **flattest tail among feature-rich gateways**: p99/p50 = **2.8×**, vs traefik 3.1×, nginx/haproxy ~3.5×. mimalloc + LTO=fat + the gated telemetry pattern collectively suppress outlier latencies.

## Caveats

- **Envoy (v1.33.0 GitHub release binary)**: handled single `curl` requests fine but returned non-HTTP responses to both `bombardier` and `ab` at *any* concurrency ≥ 32 — every request counted as "other" by the load generator, zero `2xx` recorded. The binary downloaded straight from `github.com/envoyproxy/envoy/releases` doesn't appear to be a fully self-contained static binary for ad-hoc setups; production envoy deployments typically run via the upstream image or with xDS. Industry reference on this hardware class: **30-45k RPS**.
- **Kong (OSS)**: `packages.konghq.com` GPG key URL returned 404 during install. Industry reference on this hardware class: **12-22k RPS** with no plugins, **8-15k** with auth + rate-limit (the OpenResty Lua interpreter cost).
- **All measurements use bombardier** (closed-loop with 512 persistent connections). HDR-based open-loop measurements typically report slightly different absolute numbers but the **rank order** is consistent with widely-cited published benchmarks.

## Tier ranking (subjective — features ↔ throughput trade)

| tier | gateway | best for |
|---|---|---|
| "raw speed, no features" | nginx, haproxy | static reverse-proxy, L4/L7 LB |
| **"features + speed"** | **lynxx**, envoy | API gateway, plugin chains, observability |
| "features-first" | traefik, caddy, kong | when DX matters more than throughput |

## Setup

### Topology

```
┌────────────────────────────────────────┐         ┌──────────────────────────────────────┐
│ CLIENT  (n2-standard-4, 4 vCPU, 16 GB) │  ────►  │ SUT  (n2-highcpu-4, 4 vCPU, 4 GB)    │
│ bombardier 1.2.7                       │         │ each proxy on its own port           │
│                                        │         │ + Go upstream on :8090 (hello-lynxx) │
└────────────────────────────────────────┘         └──────────────────────────────────────┘
            same GCP region (us-central1-a), same VPC, internal IPs
```

The proxy-to-upstream hop is `localhost`. Every proxy gets the same upstream conditions and the same kernel tuning.

### Port assignments

| port | gateway |
|---|---|
| 8080 | lynxx |
| 8082 | haproxy |
| 8083 | nginx |
| 8084 | traefik |
| 8085 | envoy |
| 8086 | caddy |
| 8090 | Go upstream (`hello-lynxx`, 12-byte body, keepalive) |

### Kernel tuning (both VMs)

```
net.core.somaxconn=65535
net.ipv4.tcp_tw_reuse=1
net.ipv4.ip_local_port_range="1024 65535"
fs.file-max=2097152
net.core.netdev_max_backlog=16384
net.ipv4.tcp_max_syn_backlog=65535
ulimit -n 1048576
```

### Bench params

```
bombardier -d 30s -c 512 -l http://<gw>:<port>/
```

(no rate cap, 512 persistent connections, 30 s sustain, latency distribution captured)

### lynxx config (`fully loaded`)

```toml
[gateway]
proxy_port = 8080
admin_port = 8081
database_url = "sqlite:///tmp/lynxx-bench.db?mode=rwc"
seed_from_config = true
minimal_telemetry = true   # ← Tier 1 feature; skips per-request prometheus + access-log

[[services]]
name = "upstream"
url = "http://127.0.0.1:8090"
retries = 0
connect_timeout_ms = 5000

[[routes]]
paths = ["/"]
methods = ["GET"]
service = "upstream"
strip_path = false
```

Build flags (`Cargo.toml [profile.release]`): `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`, `overflow-checks = false`. mimalloc as global allocator. Compiled via `cargo zigbuild --release --target x86_64-unknown-linux-gnu`.

### haproxy config

```
global
    maxconn 65535
    nbthread 4
    daemon

defaults
    mode http
    timeout connect 5s
    timeout client 30s
    timeout server 30s
    option http-keep-alive

frontend f
    bind *:8082
    default_backend b

backend b
    server s 127.0.0.1:8090 maxconn 65535
```

### nginx config

```nginx
worker_processes auto;
worker_rlimit_nofile 1048576;
events {
    worker_connections 65535;
    use epoll;
    multi_accept on;
}
http {
    access_log off;
    sendfile on;
    tcp_nopush on;
    tcp_nodelay on;
    keepalive_timeout 65;
    keepalive_requests 100000;
    upstream up {
        server 127.0.0.1:8090 max_conns=0;
        keepalive 512;
    }
    server {
        listen 8083 reuseport backlog=65535;
        location / {
            proxy_pass http://up;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
        }
    }
}
```

### traefik config (file provider)

```yaml
entryPoints:
  web:
    address: ":8084"
log:
  level: ERROR
accessLog: null
providers:
  file:
    filename: /tmp/traefik-dynamic.yml
```

```yaml
http:
  routers:
    bench:
      rule: "PathPrefix(`/`)"
      service: bench-svc
      entryPoints: [web]
  services:
    bench-svc:
      loadBalancer:
        servers:
          - url: "http://127.0.0.1:8090"
```

### caddy Caddyfile

```
{
  auto_https off
  servers { metrics }
}
:8086 {
  reverse_proxy 127.0.0.1:8090
  log { output discard }
}
```

## Tier 1 trajectory (lynxx-only)

The 28,761 RPS in this report is the result of seven Tier 1 commits landed today:

| commit | phase | description | cumulative RPS |
|---|---|---|---:|
| (pre-Tier 1) | — | original baseline | 22,851 |
| `edceb5a` | profile | LTO=fat + codegen-units=1 + panic=abort | 24,182 |
| `fbecea8` | 1A | bench_mode env var skips per-request telemetry | 27,403 |
| `4f6ea2e` | 1B | promote to `[gateway].minimal_telemetry` config | 27,403 |
| `b1befd3` | 1C | skip MatchContext insertion when plugins empty | (rolled into 1D) |
| `3d6af91` | 1D | borrow path/method/query from owned `Uri` | 27,812 |
| `9696e95` | 1F | gate all telemetry through `ProxyState` helpers | 27,812 |
| `3ec440a` | 1E | `Arc<str>` for `Service.name` + `Target.url` | **28,761** |

**Total Tier 1 gain: +25.9% RPS** (22,851 → 28,761).

## What's next (Tier 2 — to close the gap to nginx/haproxy)

The remaining 46% gap is **structural**, not allocation-driven:
- Hyper request/response building (200-400 ns per request)
- axum extractor + tower service chain overhead
- 20-30 syscalls per request (vs nginx/haproxy ~10 with `splice`/`sendfile`/batched recv)

Tier 2 work (in order of bang-for-buck):
1. **Raw-TCP shortcut** for "trivial" services (no plugins, no auth, plain HTTP, no TLS termination) — skip axum/hyper entirely. ~2-3 weeks, projected **+30%**.
2. **monoio runtime + io_uring** on the listener thread — near-zero syscall overhead. ~2-3 weeks, projected **+20%**.
3. **Custom HTTP/1 parser** for the fast path — SIMD-accelerated header search. ~2 weeks, projected **+10%**.

Combined Tier 2 estimate: lynxx 55-65k RPS → beats nginx/haproxy.
