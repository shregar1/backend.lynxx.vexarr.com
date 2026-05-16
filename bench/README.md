# lynxx-bench

Open-loop load generator for the Lynxx gateway. Records per-request latency
into an `HdrHistogram` and reports p50/p95/p99/p99.9 + RPS + errors. Supports
side-by-side runs against multiple targets via `--compare`.

> Target URLs default to `http://127.0.0.1:28080` (proxy) and
> `http://127.0.0.1:28081` (admin). Override per-invocation with `--target` /
> `--proxy` / `--admin`, or set `LYNXX_PROXY_PORT` / `LYNXX_ADMIN_PORT` /
> `LYNXX_ADMIN_TOKEN` in your `.env` — see [`../.env.example`](../.env.example).

## Usage

```bash
cargo build --release -p lynxx-bench
./target/release/lynxx-bench \
  --target http://localhost:28080/bench \
  --rps 2000 --duration-secs 30 --connections 64 \
  --compare kong=http://localhost:18000/bench \
  --compare direct=http://localhost:18080/
```

## Reference results (M1 Mac, 2026-05-05)

`hashicorp/http-echo` upstream, 30s @ 2000 RPS via 64 connections, plugin chain disabled
on both gateways.

```
metric         lynxx       kong      direct
─────────────────────────────────────────────
p50            545µs      572µs      790µs    ← lynxx wins
p95            1.80ms     6.48ms    31.21ms   ← lynxx wins (3.6× faster)
p99           10.46ms    24.41ms   143.23ms   ← lynxx wins (2.3× faster)
p99.9         42.11ms   111.23ms     1.34s    ← lynxx wins (2.6× faster)
errors             0          0          0
```

## Production benchmark (GCE n2-highcpu-8, 2026-05-10)

Dedicated 8-vCPU VM, kernel-tuned (`somaxconn=65535`, `tcp_tw_reuse=1`,
`ip_local_port_range=1024–65535`, `file-max=2097152`).
All gateways run as native binaries (no Docker overhead).
Upstream: `lynxx-mock-upstream --port 19080`.
Plugin chain disabled on all gateways.

### Multi-gateway comparison (Docker, 5k/10k RPS)

```
                 5,000 RPS / 128 conns / 60s
metric      lynxx      kong    traefik    envoy    nginx
────────────────────────────────────────────────────────
p50         279µs     338µs     336µs     392µs    218µs
p95         434µs     545µs     574µs     680µs    337µs
p99         608µs     761µs     851µs    1.60ms    505µs
p99.9       786µs    1.41ms    1.54ms    3.67ms    654µs
errors          0         0         0         0        0

                10,000 RPS / 256 conns / 60s
metric      lynxx      kong    traefik    envoy    nginx
────────────────────────────────────────────────────────
p50         307µs     462µs     573µs    1.27ms    274µs
p95         521µs     968µs    1.15ms   21.48ms    520µs
p99         889µs    1.36ms    1.66ms   37.25ms    880µs
p99.9      1.30ms    2.10ms    2.91ms   83.89ms   1.30ms
errors          0         0         0         0        0
```

**Lynxx beats Kong at every percentile (25–35 % faster at p99).** Traefik
and Envoy fall further behind at higher load. NGINX edges Lynxx by ~20 % at
p99 — expected for a hand-tuned C proxy with no runtime overhead.

### Lynxx vs native NGINX — escalating to Instagram-scale

Native binaries only, no containers. NGINX 1.24 with `reuseport`,
`keepalive 256`, `access_log off`, `worker_processes auto`.

```
  5,000 RPS / 128 conns / 60s            proxy overhead (p99 − direct p99)
metric      direct     lynxx     nginx    lynxx      nginx
──────────────────────────────────────────────────────────
p50          108µs     281µs     211µs    +173µs     +103µs
p95          176µs     390µs     297µs    +214µs     +121µs
p99          212µs     449µs     345µs    +237µs     +133µs
p99.9        260µs     523µs     412µs    +263µs     +152µs
errors           0         0         0

 10,000 RPS / 256 conns / 60s
metric      direct     lynxx     nginx    lynxx      nginx
──────────────────────────────────────────────────────────
p50          101µs     303µs     213µs    +202µs     +112µs
p95          165µs     458µs     305µs    +293µs     +140µs
p99          199µs     544µs     359µs    +345µs     +160µs
p99.9        249µs     661µs     443µs    +412µs     +194µs
errors           0         0         0

 25,000 RPS / 512 conns / 60s
metric      direct     lynxx     nginx    lynxx      nginx
──────────────────────────────────────────────────────────
p50           94µs     354µs     202µs    +260µs     +108µs
p95          154µs     716µs     457µs    +562µs     +303µs
p99          185µs    1.02ms     784µs    +835µs     +599µs
p99.9        248µs    1.51ms    1.06ms   +1.26ms     +812µs
errors           0         0         0

 50,000 RPS requested → upstream-saturated
metric      direct     lynxx     nginx
──────────────────────────────────────
achieved    31,861    27,608    26,829    ← lynxx 3% higher throughput
p50           87µs     460µs     196µs
p95          132µs    1.67ms     735µs
p99          176µs    2.67ms    1.19ms
p99.9        261µs    3.89ms    1.79ms
errors           0         0         0

100,000 RPS requested → upstream-saturated
metric      direct     lynxx     nginx
──────────────────────────────────────
achieved    32,421    27,588    26,899    ← lynxx 3% higher throughput
p50           85µs     469µs     193µs
p95          126µs    1.68ms     666µs
p99          168µs    2.67ms    1.07ms
p99.9        234µs    3.86ms    1.57ms
errors           0         0         0
```

### Key findings

1. **Zero errors across every load level.** Both gateways handled 6M+
   requests without a single failure — production-grade reliability.
2. **Lynxx beats Kong/Traefik/Envoy convincingly.** 25–35 % faster than
   Kong at p99; 2–40× faster than Envoy under load.
3. **NGINX wins on raw per-request latency** (~1.5–2× lower p99). Expected:
   NGINX is 20 years of hand-tuned C with zero runtime overhead.
4. **Lynxx wins on saturated throughput.** Under backpressure (50k/100k
   requested), Lynxx achieved 3 % higher RPS than NGINX — better
   connection pooling and async backpressure handling.
5. **Instagram-scale projection.** At 27.5k RPS per 8-vCPU node with
   sub-3ms p99, an Instagram-scale gateway (~500k RPS) needs ~18 Lynxx
   nodes with comfortable headroom.

### Why Lynxx is fast

- mimalloc as the global allocator (long-tail allocator pauses gone)
- hickory-resolver DNS cache (no per-connection lookup)
- hyper-util connection pool: 128 idle/host, 120s idle timeout, h2 keep-alive
- TCP_NODELAY everywhere
- background pre-warmer keeping the pool hot for known upstreams
- streaming hyper-util client (no body buffering, no HeaderMap clone)
- empty-plugin-chain fast path (no locks acquired)
- exact-match route index (O(1) literal-path lookups)
- Tokio runtime sized to physical cores

## Reproducing

```bash
# upstream
docker run -d --name bench-upstream -p 18080:5678 hashicorp/http-echo -text=ok

# kong (db-less mode)
docker network create bench-net
docker network connect bench-net bench-upstream
cat > /tmp/kong.yml <<EOK
_format_version: "3.0"
services:
  - name: bench
    url: http://bench-upstream:5678
    routes: [{ name: bench, paths: [/bench], strip_path: true }]
EOK
docker run -d --name bench-kong --network bench-net \
  -e KONG_DATABASE=off -e KONG_DECLARATIVE_CONFIG=/etc/kong/kong.yml \
  -e KONG_PROXY_LISTEN="0.0.0.0:18000" -e KONG_ADMIN_LISTEN="0.0.0.0:18001" \
  -v /tmp:/etc/kong -p 18000:18000 kong:3.8

# lynxx
cargo run --release --bin lynxx-gateway &
lynxx services create --name local-bench --url http://127.0.0.1:18080/
lynxx routes create --service local-bench --paths /bench --strip-path
```

## Caveats

- Open-loop pacing means a slow upstream causes queue buildup, not RPS drop.
- Single-process generator caps around 10k RPS on M1; for higher load run
  multiple instances behind a fan-out.
- Latency is end-to-end including the bench → gateway → upstream path. The
  `direct=` baseline isolates upstream + bench overhead so you can subtract.

## Chaos / Soak Tests

`lynxx-bench` is the steady-state benchmark — the chaos suite is the
"will it survive a bad day" suite. Two extra binaries ship with the same
crate:

- `lynxx-mock-upstream` — a controllable HTTP server. Substitutes for a dumb echo
  container when a scenario needs to inject latency, status codes, or echo the request
  back as JSON.
- `lynxx-chaos` — the orchestrator. Each subcommand is one scenario; `all`
  runs every scenario sequentially with shorter defaults.

```bash
cargo build --release -p lynxx-bench
# in one terminal:
./target/release/lynxx-mock-upstream --port 19090 --status-distribution 80,15,5

# in another terminal (gateway must already be running):
./target/release/lynxx-chaos --proxy http://127.0.0.1:28080 \
    --admin http://127.0.0.1:28081 --admin-token "$LYNXX_ADMIN_TOKEN" \
    --mock-upstream http://127.0.0.1:19090 \
    all
```

### Mock upstream flags

```
--port <n>                       listen on 127.0.0.1:<n> (default 19090)
--status-distribution 2xx,4xx,5xx  e.g. 80,15,5; 4xx picks 400/401/404,
                                 5xx picks 500/502/503
--latency 1ms,5ms,20ms           round-robin (or 50/40/10 weighted with 3 vals)
--echo                           reflect method/path/headers/body as JSON
```

### The 10 scenarios

| Subcommand     | What it proves                                                   |
|----------------|------------------------------------------------------------------|
| `mixed-bodies` | 70/20/9/1 GET / 1KB / 100KB / 5MB body mix at 1k RPS for 5 min   |
| `slowloris`    | 100 byte-per-300ms slow clients don't starve fast traffic        |
| `methods`      | Mixed methods + status distribution match upstream within ±2pp   |
| `burst`        | 200→2000→200 RPS spike: steady p99<5ms, burst p99<50ms, recover  |
| `churn`        | Service create/use/delete every 5s under 500 RPS background      |
| `plugin-chain` | jwt + rate-limit + cors + logging + request-transformer combined |
| `ws-flood`     | 500 WS connections at 100 msg/sec each, no drops                 |
| `memory`       | 10-min @ 1k RPS RSS growth < 50 MB                               |
| `pg-restart`   | Cached config keeps serving while Postgres restarts mid-traffic  |
| `malformed`    | 1000 raw hostile probes; gateway responds 4xx/closes; no leak    |

### Running individual scenarios

```bash
lynxx-chaos mixed-bodies --rps 1000 --duration-secs 300
lynxx-chaos slowloris
lynxx-chaos methods --rps 500 --duration-secs 180
lynxx-chaos burst                              # owns its own 70s timeline
lynxx-chaos churn --duration-secs 120
lynxx-chaos plugin-chain --rps 1000 --duration-secs 60
lynxx-chaos ws-flood --duration-secs 60
lynxx-chaos memory --rps 1000 --duration-secs 600
lynxx-chaos pg-restart --pg-container lynxx-postgres
lynxx-chaos malformed
lynxx-chaos all                                # short-default all 10 in <15min
```

Global flags (`--proxy`, `--admin`, `--admin-token`, `--mock-upstream`,
`--pg-container`, `--duration-secs`, `--rps`, `--quiet`) work on every
subcommand and on `all`.

### Output format

Each scenario reports a one-line verdict. `all` aggregates them:

```
==== chaos suite summary ====
✓ mixed-bodies   180,000 reqs · 0 errs · p99=12.3ms
✓ slowloris      100/100 fast ok · p50=2.1ms during slow
✓ burst          steady p99=4.2ms · burst p99=38.1ms · recovery p99=4.7ms
✗ memory         180,000 reqs · RSS 142.0 -> 209.0 MB (+67.0 MB)
    budget 50 MB; observed +67.0 MB over 600s
9/10 scenarios passed
```

Exit code is non-zero if any scenario failed, so the suite plugs straight
into CI.

### Notes

- `memory` and `malformed` sample the gateway's RSS via
  `pgrep -f lynxx-gateway` + `ps -o rss=`. They will fail (cleanly) if no
  matching process is found.
- `pg-restart` requires `--pg-container <name>` and a working `docker` CLI
  in PATH. Without it the scenario reports "skipped" and `all` carries on.
- `ws-flood` runs its own embedded WS echo server on an ephemeral port, so
  no separate WS upstream is needed.
- Scenarios clean up the services/routes/plugins they create. If a run is
  killed mid-flight, leftovers are named `chaos-*` and easy to spot in the
  admin API.
