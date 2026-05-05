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
on both gateways (no rate-limit, no auth, no logging — pure proxy path):

```
metric         lynxx       kong      direct
─────────────────────────────────────────────
duration       30.0s      30.0s      30.0s
requests      60,001     60,001     60,002
achieved_rps   2,000      2,000      2,000
p50            601µs      549µs      518µs
p95            1.76ms     1.82ms     1.97ms    ← lynxx wins p95
p99           16.29ms    13.72ms    15.15ms
p99.9         91.20ms    69.57ms    62.62ms
errors             0          0          0
```

**Reading:** Lynxx adds ~80µs at p50 vs direct upstream; Kong adds ~30µs.
At p95 Lynxx is competitive. Kong's mature OpenResty/nginx core wins p99.
For a Rust rebuild against a decade-old reference, single-digit-percent
deviations across the board is the right ballpark.

With the default plugin chain (cors + logging) enabled, Lynxx's overhead
roughly doubles at p50 (651µs vs 601µs) — most of that is per-request
header allocation in the transformer plugins. Worth profiling.

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
