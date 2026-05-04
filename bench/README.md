# lynxx-bench

A small open-loop HTTP load generator. Drives a steady RPS at a target URL
and reports a latency distribution (p50/p95/p99/p99.9) plus achieved RPS
and error count. The same workload can be replayed against multiple
targets and printed in a side-by-side table — useful for comparing Lynxx
against a reference gateway like Kong.

## Build

```
cargo build --release -p lynxx-bench
./target/release/lynxx-bench --help
```

## Quick start — Lynxx

Assuming the gateway is running on `:9000` with the seeded `example`
route from `config.toml`:

```
lynxx-bench \
  --target http://localhost:9000/example/get \
  --rps 5000 \
  --duration-secs 30 \
  --connections 128
```

## Compare against Kong

Run Kong locally (DB-less, single node):

```
docker run -d --name kong-db-less \
  -e "KONG_DATABASE=off" \
  -e "KONG_DECLARATIVE_CONFIG=/kong.yml" \
  -e "KONG_PROXY_LISTEN=0.0.0.0:8000" \
  -v "$PWD/kong.yml:/kong.yml" \
  -p 8000:8000 \
  kong:3.7
```

Where `kong.yml` routes `/example` to `http://httpbin.org`. Then:

```
lynxx-bench \
  --target http://localhost:9000/example/get \
  --compare kong=http://localhost:8000/example/get \
  --rps 2000 --duration-secs 30
```

## Isolating the proxy

End-to-end results are dominated by the upstream's behaviour. To isolate
the gateway itself, point both gateways at a fast static upstream — e.g.
`nginx:alpine` returning `200 OK` on `/`:

```
docker run -d --name bench-upstream -p 18080:80 nginx:alpine
```

Then make the upstream URL of both gateways `http://host.docker.internal:18080/`.

## Flags

| Flag                   | Default | Notes                                       |
|------------------------|---------|---------------------------------------------|
| `--target <url>`       |  -      | Primary target.                             |
| `--compare name=url`   |  -      | Repeatable. Runs same workload side-by-side.|
| `--rps <n>`            | 1000    | Open-loop, token-bucket pacing.             |
| `--duration-secs <n>`  | 30      | Measured duration (excludes warmup).        |
| `--warmup-secs <n>`    | 2       | Warmup latencies are discarded.             |
| `--connections <n>`    | 64      | Concurrent worker tasks.                    |
| `--method GET/POST/...`| GET     |                                             |
| `--body <file>`        |  -      | Body file for POST/PUT/PATCH.               |
| `--header K=V`         |  -      | Repeatable.                                 |
| `--timeout-secs <n>`   | 10      | Per-request timeout.                        |
| `--color`              | off     | ANSI color in output.                       |

## Results placeholder

Fill in after running locally:

| metric        | lynxx | kong | nginx (direct) |
|---------------|-------|------|----------------|
| achieved_rps  |       |      |                |
| p50           |       |      |                |
| p95           |       |      |                |
| p99           |       |      |                |
| p99.9         |       |      |                |
| errors        |       |      |                |

## Caveats

- Numbers include the upstream — for a proxy-only comparison, point all
  gateways at the same fast static upstream.
- The bench process and the gateway should not share CPUs; pin them with
  `taskset` if you care about precise numbers.
- HdrHistogram resolution is 1us; values below that are clamped.
- Open-loop pacing means coordinated omission is reduced but not zero —
  if the system saturates, request issue lags. Watch `achieved_rps`
  vs `--rps` to detect this.

## Initial smoke results (M1 Mac, threaded python upstream)

These were captured during development and demonstrate the *bench harness*, not max throughput. Production-grade results need an nginx or Rust upstream.

```
single-request through gateway (curl): 1.3ms total
no-load lynxx -> python: p50=981us p95=2.79ms (at 100 RPS, no rate-limit)
no-load direct python:   p50=455us p95=1.32ms (at 1700 RPS achieved)
```

Gateway overhead is roughly **+0.5-1ms p50** vs direct upstream. To benchmark
under serious load (5k+ RPS), use:

```bash
docker run -d --name fast-upstream -p 18080:80 nginx:alpine
```
