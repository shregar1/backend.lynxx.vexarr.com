# Supply chain: signed releases + transparency log

How a `curl https://backend.lynxx.vexarr.com/install.sh | sh` is verified
before it touches the user's disk. Same scheme used by all 27 X-products
in the Vexarr fleet — just swap the bucket name and product-scoped env vars.

If you are reading this in 6 months trying to remember what
`gs://vexarr-transparency-log/by-sha256/…` is: keep reading.

---

## The four verification layers

Every artifact served to an install script is checked against four
independent signals. Failure of any one is a hard install refusal.

| L | Check                         | Defends against                              | Stored at                                          |
|---|-------------------------------|----------------------------------------------|----------------------------------------------------|
| 1 | SHA-256                       | Bucket bit-flip, accidental overwrite, MITM  | `<bin>.sha256` sidecar (shasum format)             |
| 2 | Ed25519 signature of binary   | Bucket compromise, gateway-side substitution | `<bin>.sig` sidecar (raw 64-byte sig)              |
| 3 | Transparency log entry        | Off-the-record signing                       | `gs://vexarr-transparency-log/by-sha256/<sha>/…`   |
| 4 | Witness-signed root inclusion | State-actor with the release key             | `gs://vexarr-transparency-log/roots/latest-root.*` |

L1 alone gives transport integrity. L1+L2 closes anyone-without-the-private-key.
L1+L2+L3 forces every release to leave a public audit trail. L1+L2+L3+L4
makes the attack require compromising two different security domains.

The first three are mandatory; L4 is soft-fail by default and hard-fail
under `LYNXX_REQUIRE_WITNESS=1` (or `<PRODUCT>_REQUIRE_WITNESS=1` in
non-lynxx products).

---

## The two keys

Two **independent** Ed25519 keypairs. Their independence is the whole
security argument — keep it real, not theatrical.

| Role           | Secret Manager name                | Fingerprint (sha256 of pubkey) |
|----------------|------------------------------------|--------------------------------|
| Release signer | `vexarr-release-signing-key`       | `fd932963…`                    |
| Log witness    | `vexarr-log-witness-key`           | `e38dce76…`                    |

**Public keys** are pinned literally into every install script as PEM
constants (`*_RELEASE_PUBKEY`, `*_WITNESS_PUBKEY`). Rotating either key
means regenerating all 90 install scripts (see "Rotation" below).

**Private keys** live in GCP Secret Manager. The release key may also
sit at `~/.config/vexarr-releases/signing.key` on the operator's laptop
for local development — production CI fetches via `gcp-secret://`.

### Why two keys

A single-key system has one bad day: the day the key leaks. Then an
attacker can sign any binary, sign any log entry covering it, and the
install scripts have nothing to disbelieve.

With the witness as a second key in a different security domain:

- Compromising **only** the release key → attacker can sign binaries and
  log entries, but cannot produce a fresh witness-signed root that
  includes their forged sha256. Install refuses at L4.
- Compromising **only** the witness key → attacker can sign arbitrary
  log roots, but has no way to sign a binary that L2 accepts. Install
  refuses at L2.
- Compromising **both** → trust boundary breached. By design, getting
  here requires reaching across two operator/IAM domains.

The "different security domains" part is **operational policy**, not
something the code enforces. As of writing both secrets are reachable
from the same gcloud identity (the developer laptop). To realise the
design's actual value:

```
# Bind release-signing-key to ONLY the CI release service account
gcloud secrets add-iam-policy-binding vexarr-release-signing-key \
  --member=serviceAccount:ci-release@…iam.gserviceaccount.com \
  --role=roles/secretmanager.secretAccessor

# Bind witness-key to ONLY a separate witness-runner service account
gcloud secrets add-iam-policy-binding vexarr-log-witness-key \
  --member=serviceAccount:witness-runner@…iam.gserviceaccount.com \
  --role=roles/secretmanager.secretAccessor
```

No human or service should have `secretAccessor` on both.

---

## What an install request looks like

```
curl -fsSL https://backend.lynxx.vexarr.com/install.sh | sh
                  │
                  ▼
gateway: GET /install.sh
  → returns embedded script body (assets/install/install.sh,
    bundled into the binary via include_str!)

script: detect OS/arch, call /v1/install/binary?artifact=cli&os=linux&arch=amd64
                  │
                  ▼
gateway: stream gs://lynxx-public/cli/linux/amd64/lynxx
  → headers:  X-Content-SHA256, X-Content-Signature
  → body:     binary bytes (streamed; no buffering)
  → bucket auth: gateway's GCE-metadata-server SA token
                  │
                  ▼
script: shasum -a 256 → compare to X-Content-SHA256        (L1)
script: openssl pkeyutl -verify -pubin … (release pubkey)  (L2)

script: GET /v1/install/transparency?sha256=<hex>
                  │
                  ▼
gateway: list gs://vexarr-transparency-log/by-sha256/<sha>/
  → fetch lex-max .json + .sig
  → return body=entry-json, X-Transparency-Signature=base64
                  │
                  ▼
script: openssl pkeyutl -verify (release pubkey) on entry  (L3 entry-sig)
script: grep entry.sha256 == download sha256                (L3 sha-match)
script: grep entry.sig   == binary sig                      (L3 sig-match)

script: GET /v1/install/transparency-root
                  │
                  ▼
gateway: fetch gs://vexarr-transparency-log/roots/latest-root.json{,.sig}
  → return body=root-json, X-Witness-Signature=base64
                  │
                  ▼
script: openssl pkeyutl -verify (witness pubkey) on root   (L4 witness-sig)
script: grep "<expected_sha>" inside root.entries           (L4 inclusion)

script: install binary to /usr/local/bin/lynxx
```

Mockx is the same flow in Go (`internal/handler/install.go`). Every other
gateway is generated from the template at `/tmp/x-port-install-v2.sh`
(promote to a real path before you rely on it long-term).

---

## Publishing a release

`scripts/upload-artifact.sh` is the only blessed path. It produces all
the sidecars + the transparency log entry in one call.

```bash
VEXARR_SIGNING_KEY=gcp-secret://vexarr-release-signing-key \
  scripts/upload-artifact.sh \
    --binary  target/x86_64-unknown-linux-gnu/release/lynxx-gateway \
    --dest    gs://lynxx-public/gateway/linux/amd64 \
    --name    lynxx-gateway \
    --version v0.1.0-$(git rev-parse --short=8 HEAD) \
    --meta    os=linux --meta arch=amd64 --meta commit=$(git rev-parse --short=8 HEAD)
```

What this does:

1. `gs://…/lynxx-gateway-<version>`            (versioned, immutable)
2. `gs://…/lynxx-gateway-<version>.sha256`
3. `gs://…/lynxx-gateway-<version>.sig`
4. `gs://…/lynxx-gateway`                       (stable pointer, mutable)
5. `gs://…/lynxx-gateway.sha256`
6. `gs://…/lynxx-gateway.sig`
7. `gs://vexarr-transparency-log/by-sha256/<sha>/<ts>.json` + `.sig`

Without `--signing-key`, only steps 1, 2, 4, 5 happen (unsigned legacy
mode). Install scripts gracefully degrade with a visible warning.

Local-file key path also works: `--signing-key ~/.config/vexarr-releases/signing.key`.

---

## Running the witness

The transparency log is meaningless without somebody periodically
signing log roots. **This must run on a different machine + identity
than the release pipeline.**

```bash
VEXARR_WITNESS_KEY=gcp-secret://vexarr-log-witness-key \
  scripts/witness-log.sh --apply
```

Wire it into Cloud Scheduler / cron / GitHub Actions cron on whatever
infra the witness-runner SA has access to. Hourly is plenty; tying it
to release events is even better (a release isn't "covered" until the
next witness pass runs).

Dry-run first to see what would be signed:

```bash
scripts/witness-log.sh
# prints the canonical root that would be published, plus entry count
```

A root file looks like:

```json
{"v":1,
 "entries":["94fb430deb1f97d0…"],
 "entries_count":1,
 "entries_digest":"5a22c32a388d5290…",
 "timestamp":"2026-05-16T15:40:36Z",
 "witness":"vexarr-witness-v1"}
```

The matching `.sig` is a 64-byte raw Ed25519 signature over those bytes.

---

## Rotation

`scripts/rotate-signing-key.sh` for the **release** key. (No equivalent
yet for the witness key — same procedure, harder to script because the
witness only ever publishes new roots; old roots are immutable history.)

```bash
# Plan only — prints what would happen, writes a runbook to /tmp/
scripts/rotate-signing-key.sh

# Execute the rotation
scripts/rotate-signing-key.sh --apply --window-days 14

# After the window, drop the old key everywhere
scripts/rotate-signing-key.sh --apply --finalise
```

Rotation is **not zero-cost**: every install script has the pubkey
literally pinned. Mid-rotation, scripts accept either key; post-
finalise, only the new one. Plan deployments accordingly.

For the witness key, the steps are similar but you must:
1. Generate new witness key, push to a new Secret Manager version
2. Update `*_WITNESS_PUBKEY` in `/tmp/x-product-setup.sh` to a
   two-PEM concatenation
3. Regen all 90 install scripts
4. Run `witness-log.sh --apply` with the new key — produces a fresh
   root signed by the new key, while old roots remain history
5. After the dual-key window, drop the old witness pubkey

---

## Failure modes — what does each layer look like when it fires

Install scripts say specifically which layer rejected, so a user
reporting "install failed" tells you immediately where the gap is.

| Symptom (printed by install.sh)                                        | Cause                                                |
|------------------------------------------------------------------------|------------------------------------------------------|
| `checksum mismatch — expected … got …`                                 | L1 fail — bucket was tampered after upload           |
| `no X-Content-SHA256 header`                                           | Legacy unsigned artifact (warn only)                 |
| `signature verification FAILED`                                        | L2 fail — wrong key signed, or binary modified       |
| `openssl lacks Ed25519 support`                                        | LibreSSL on macOS — install OpenSSL 3 (warn only)    |
| `binary not in transparency log`                                       | L3 fail — release key used off the record (404)      |
| `transparency log sha256 mismatch`                                     | L3 fail — entry forged for a different binary        |
| `transparency log binary-signature mismatch`                           | L3 fail — entry claims a different sig than served   |
| `no witness-signed root available`                                     | Witness cron hasn't run yet (warn unless REQUIRE=1)  |
| `witness root signature INVALID`                                       | L4 fail — root was not signed by the witness key     |
| `sha256 not in witness-signed root — off-the-record signing detected`  | L4 fail — the **state-actor** scenario               |

---

## What this design does **not** defend against

Be honest about the boundaries.

- **The same operator controlling both Secret Manager bindings.** Until
  the IAM split above is done, "two keys" is technically true but
  operationally indistinguishable from one key.
- **A compromised build pipeline.** We sign whatever bytes we're handed.
  If the CI runner is poisoned, we sign poisoned bytes and the
  transparency log faithfully records the poisoning. Reproducible
  builds + source attestation (SLSA-style) are the next layer up.
- **Coercion of the entire org.** No cryptographic scheme defends against
  Vexarr Inc. being legally compelled to sign + log a backdoored binary.
  The defense at that point is external transparency — cross-publishing
  to a public CT log (Sigstore Rekor, Trillian) so non-Vexarr observers
  can audit. Not built yet.
- **Replay of old binaries.** Nothing stops an attacker from re-serving
  a real but stale binary (one with a known CVE). Install scripts don't
  check freshness. Add a maximum-age check on entry.timestamp if this
  matters.

---

## Where the bits live

| Thing                                  | Path                                                                          |
|----------------------------------------|-------------------------------------------------------------------------------|
| Install module (Rust)                  | `src/install/mod.rs` in each axum gateway                                     |
| Install module (Go, mockx only)        | `mockx.dev/api.mockx.dev/internal/handler/install.go`                         |
| Bundled install scripts                | `assets/install/install*.{sh,ps1}` in each gateway repo                       |
| Upload script (canonical)              | `backend.lynxx.vexarr.com/scripts/upload-artifact.sh`                         |
| Upload script (45 propagated copies)   | `<each-product-repo>/scripts/upload-artifact.sh`                              |
| Witness script                         | `backend.lynxx.vexarr.com/scripts/witness-log.sh`                             |
| Rotation script + runbook generator    | `backend.lynxx.vexarr.com/scripts/rotate-signing-key.sh`                      |
| Install-script generator (regen all 86)| `/tmp/x-product-setup.sh` (move to a real path before you depend on it)       |
| Rust install-module generator          | `/tmp/x-port-install-v2.sh` (same caveat)                                     |
| Release key (private)                  | Secret Manager `vexarr-release-signing-key`; mirror at `~/.config/vexarr-releases/signing.key` |
| Witness key (private)                  | Secret Manager `vexarr-log-witness-key`; mirror at `~/.config/vexarr-releases/witness.key`     |
| Both public keys                       | Embedded as PEM constants in every install script                             |
| Transparency log                       | `gs://vexarr-transparency-log/` (public-read, `by-sha256/` + `roots/`)        |
| Per-product release buckets            | `gs://<product>-public/` or `gs://<product>-releases/` (private, gateway-SA reads only) |
| Per-product gateway service accounts   | `<product>-gateway@vexarr-com-495315.iam.gserviceaccount.com`                 |

---

## TL;DR for an incident

If something is wrong on a customer install:

1. **Get the URL they curl'd and the exact error message.** That alone
   tells you which layer (see the table above).
2. **Re-download the artifact + sidecars by hand:**
   ```bash
   gcloud storage cp gs://<bucket>/<path>/<bin>{,.sha256,.sig} /tmp/
   ```
3. **Run the same openssl verify the script would:**
   ```bash
   /opt/homebrew/opt/openssl@3/bin/openssl pkeyutl -verify \
     -pubin -inkey ~/.config/vexarr-releases/signing.pub \
     -rawin -in /tmp/<bin> -sigfile /tmp/<bin>.sig
   ```
4. **Look up the log entry:**
   ```bash
   gcloud storage ls "gs://vexarr-transparency-log/by-sha256/<sha>/"
   ```
5. **Look up the witness root + check inclusion:**
   ```bash
   gcloud storage cat gs://vexarr-transparency-log/roots/latest-root.json \
     | grep <sha>
   ```

If steps 3–5 all pass and the install script still fails, the bug is
in the install script itself — not the supply chain. Most likely
openssl-version or path-escaping. The script always prints which check
fired; trust it.
