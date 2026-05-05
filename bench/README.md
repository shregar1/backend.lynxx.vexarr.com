# lynxx-bench

Open-loop load generator for the Lynxx gateway. Records per-request latency
into an `HdrHistogram` and reports p50/p95/p99/p99.9 + RPS + errors. Supports
side-by-side runs against multiple targets via `--compare`.

## Usage

```bash
cargo build --release -p lynxx-bench
./target/release/lynxx-bench \
  --target http://localhost:9000/bench \
  --rps 2000 --duration-secs 30 --connections 64 \
  --compare kong=http://localhost:18000/bench \
  --compare direct=http://localhost:18080/
```

## Reference results (M1 Mac, 2026-05-05)

`nginx:alpine` upstream, 30s @ 2000 RPS via 64 connections, plugin chain disabled
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

**Lynxx beats Kong on every latency percentile.** The gap widens at higher
percentiles where Kong's nginx event-loop variance shows up more.

The wins came from:
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
docker run -d --name bench-upstream -p 18080:80 nginx:alpine

# kong (db-less mode)
docker network create bench-net
docker network connect bench-net bench-upstream
cat > /tmp/kong.yml <<EOK
_format_version: "3.0"
services:
  - name: bench
    url: http://bench-upstream
    routes: [{ name: bench, paths: [/bench], strip_path: true }]
EOK
docker run -d --name bench-kong --network bench-net \
  -e KONG_DATABASE=off -e KONG_DECLARATIVE_CONFIG=/etc/kong/kong.yml \
  -e KONG_PROXY_LISTEN="0.0.0.0:18000" -e KONG_ADMIN_LISTEN="0.0.0.0:18001" \
  -v /tmp:/etc/kong -p 18000:18000 kong:3.8

# lynxx
cargo run --release --bin lynx-gateway &
lynxx services create --name local-bench --url http://127.0.0.1:18080
lynxx routes create --service local-bench --paths /bench --strip-path
```

## Caveats

- Open-loop pacing means a slow upstream causes queue buildup, not RPS drop.
- Single-process generator caps around 10k RPS on M1; for higher load run
  multiple instances behind a fan-out.
- Latency is end-to-end including the bench → gateway → upstream path. The
  `direct=` baseline isolates upstream + bench overhead so you can subtract.
