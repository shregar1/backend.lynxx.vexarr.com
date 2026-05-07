# Lynxx Gateway

API gateway in Rust. Three binaries: `lynxx-gateway` (proxy + admin :9000/:9001), `lynxx-ingress` (K8s controller), `lynxx` (admin CLI).

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
cargo run --bin lynxx-gateway
```

## CLI

The `lynxx` binary is the official admin client. It defaults to
`http://localhost:9001` and resolves the admin URL as
`--admin > LYNXX_ADMIN > ~/.config/lynxx/config.toml > default`. Tables are
boxed in light box-drawing chars on a TTY; `--output json|yaml` for machine
consumption. Color is on by default; disable with `--no-color` or `NO_COLOR=1`.
Truncated 8-char UUIDs are shown by default; `--full-id` for the whole thing.

```sh
# Inspect
lynxx ping
lynxx services list

# Manage
lynxx services create --name api --url http://api.svc:8080
lynxx routes create --service api --paths /api
lynxx plugins install jwt --config '{"secret":"shh"}'

# Declarative — diff plan, confirm, apply
lynxx dump > state.yaml
vim state.yaml
lynxx validate -f state.yaml          # offline lint
lynxx apply -f state.yaml             # prints +/~/- plan, prompts y/N
lynxx apply -f state.yaml --auto-approve  # CI mode
lynxx apply -f state.yaml --dry-run       # plan only, no mutation

# Live views (Ctrl-C exits, cursor restored)
lynxx watch services -n 2             # redraws every 2s
lynxx watch health
lynxx tail                            # per-tick request deltas from /metrics

# Smoke test a route through the proxy
lynxx exec /api/v1/users
lynxx exec /api/v1/users -X POST --body @payload.json -H 'Authorization: Bearer …'

# Local config (so you don't retype --admin)
lynxx config set admin http://prod-gateway:9001
lynxx config show
```

Apply ordering is **services → targets → routes → plugins**, with deletes in
reverse. On failure, execution stops and partial state is **not** rolled back —
the plan output makes it clear which step failed.

Shell completions: `lynxx completion zsh > _lynxx`.

## Gateway flags

The gateway accepts CLI flags as the highest-precedence config layer
(`flag > env > config.toml > default`):

```sh
lynxx-gateway --config /etc/lynxx/config.toml --proxy-port 9000 --admin-port 9001
```

