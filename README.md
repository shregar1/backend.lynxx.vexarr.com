# Lynxx Gateway

API gateway in Rust. Three binaries: `lynx-gateway` (proxy + admin :9000/:9001), `lynxx-ingress` (K8s controller), `lynxx` (admin CLI).

## Quickstart (Docker Compose)

```sh
docker compose up --build
curl http://localhost:9001/status
```

## Quickstart (Helm)

```sh
helm install lynxx ./deploy/helm/lynxx
```

## Local dev

```sh
cargo run --bin lynx-gateway
```

## CLI

The `lynxx` binary is a thin admin client. It defaults to `http://localhost:9001`
and respects `LYNXX_ADMIN`. Output is a clean monochrome table by default; pass
`--output json` or `--output yaml` for machine consumption.

```sh
# Inspect
lynxx ping
lynxx services list

# Manage
lynxx services create --name api --url http://api.svc:8080
lynxx routes create --service api --paths /api
lynxx plugins install jwt --config '{"secret":"shh"}'

# Declarative
lynxx dump > state.yaml
vim state.yaml
lynxx apply -f state.yaml
```

Shell completions: `lynxx completion zsh > _lynxx`.

## Gateway flags

The gateway accepts CLI flags as the highest-precedence config layer
(`flag > env > config.toml > default`):

```sh
lynx-gateway --config /etc/lynxx/config.toml --proxy-port 9000 --admin-port 9001
```

