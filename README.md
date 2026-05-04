# Lynxx Gateway

API gateway in Rust. Two binaries: `lynx-gateway` (proxy + admin :9000/:9001), `lynxx-ingress` (K8s controller).

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
