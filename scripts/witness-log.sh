#!/usr/bin/env bash
# Vexarr transparency-log witness.
#
# Lists every entry currently in gs://vexarr-transparency-log/by-sha256/,
# builds a canonical log root (timestamp + sorted entry hashes + count),
# signs it with the witness Ed25519 key, and publishes to
# gs://vexarr-transparency-log/roots/<ts>-root.json{,.sig}.
#
# Operational intent: the witness key lives in a DIFFERENT security domain
# than the release-signing key (separate Secret Manager secret, ideally
# different IAM principals, ideally different operator). A state actor that
# compromises only the release-signing key + the bucket can forge artifact
# signatures and unwitnessed log entries, but cannot produce a NEW signed
# root that "covers" their forged entry. Install scripts that fetch the
# latest witness-signed root and verify inclusion will reject artifacts not
# covered by a recent root.
#
# Usage:
#   scripts/witness-log.sh                                # dry-run plan
#   scripts/witness-log.sh --apply                        # publish a new root
#   VEXARR_WITNESS_KEY=gcp-secret://vexarr-log-witness-key \
#     scripts/witness-log.sh --apply
#
# Recommended: run on a cron (every 1h or every release event), from a host
# that has ONLY witness-key access, not release-signing-key access.

set -euo pipefail

APPLY=0
LOG_BUCKET="gs://vexarr-transparency-log"
WITNESS_KEY="${VEXARR_WITNESS_KEY:-${HOME}/.config/vexarr-releases/witness.key}"
PROJECT="${VEXARR_PROJECT:-vexarr-com-495315}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --apply)        APPLY=1; shift ;;
    --witness-key)  WITNESS_KEY="$2"; shift 2 ;;
    --log-bucket)   LOG_BUCKET="$2"; shift 2 ;;
    -h|--help)      sed -n '2,/^$/p' "$0" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

# Find an Ed25519-capable openssl.
OSSL=""
for c in /opt/homebrew/opt/openssl@3/bin/openssl /usr/local/opt/openssl@3/bin/openssl openssl; do
  if command -v "$c" >/dev/null 2>&1 && "$c" version 2>/dev/null | grep -q "^OpenSSL"; then
    OSSL="$c"; break
  fi
done
[[ -n "$OSSL" ]] || { echo "no OpenSSL 3+ found (LibreSSL doesn't support Ed25519)" >&2; exit 1; }

# Resolve witness key: filesystem path OR gcp-secret:// URI.
RESOLVED_KEY=""
cleanup() { [[ -n "$RESOLVED_KEY" && "$RESOLVED_KEY" == /tmp/* ]] && rm -f "$RESOLVED_KEY"; }
trap cleanup EXIT

case "$WITNESS_KEY" in
  gcp-secret://*)
    name="${WITNESS_KEY#gcp-secret://}"
    version="latest"
    [[ "$name" == *@* ]] && { version="${name#*@}"; name="${name%@*}"; }
    RESOLVED_KEY=$(mktemp); chmod 600 "$RESOLVED_KEY"
    gcloud secrets versions access "$version" --secret="$name" --project="$PROJECT" > "$RESOLVED_KEY" 2>/dev/null || {
      echo "failed to fetch witness key from Secret Manager: $WITNESS_KEY" >&2; exit 1;
    }
    ;;
  *)
    [[ -f "$WITNESS_KEY" ]] || { echo "witness key not found: $WITNESS_KEY" >&2; exit 1; }
    RESOLVED_KEY="$WITNESS_KEY"
    ;;
esac

echo "==> Witness key:    $WITNESS_KEY"
echo "==> Log bucket:     $LOG_BUCKET"
echo "==> Apply:          $([[ "$APPLY" -eq 1 ]] && echo yes || echo "NO -- dry-run")"

# --- Enumerate entries currently in the log -----------------------------
# Object names look like by-sha256/<64-hex>/<iso-ts>.json
echo ""
echo "==> Listing log entries..."
ENTRIES=$(gcloud storage ls "${LOG_BUCKET}/by-sha256/" 2>/dev/null | \
  sed -n "s|^${LOG_BUCKET}/by-sha256/\([0-9a-f]\{64\}\)/$|\1|p" | \
  sort -u)

ENTRY_COUNT=$(printf '%s\n' "$ENTRIES" | grep -c . || true)
echo "  found $ENTRY_COUNT distinct sha256 entries"

if [[ "$ENTRY_COUNT" -eq 0 ]]; then
  echo "  log is empty — nothing to witness"
  exit 0
fi

# --- Build the canonical root JSON -------------------------------------
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Build the JSON array of entries, comma-separated, fixed quoting.
ENTRIES_ARRAY=$(printf '%s\n' "$ENTRIES" | awk 'NF { printf "%s\"%s\"", (NR>1 ? "," : ""), $0 }')

# Compute a digest over the canonical entries list for forensic compactness
# (lets a third-party witness re-derive the same digest from raw log state).
ENTRIES_DIGEST=$(printf '%s\n' "$ENTRIES" | sort | sha256sum | cut -d' ' -f1)

# Canonical root JSON. Fixed key order. No whitespace. Keys (after "v")
# alphabetised. This is the byte string we sign.
ROOT_JSON=$(printf '{"v":1,"entries":[%s],"entries_count":%d,"entries_digest":"%s","timestamp":"%s","witness":"vexarr-witness-v1"}' \
  "$ENTRIES_ARRAY" "$ENTRY_COUNT" "$ENTRIES_DIGEST" "$TS")

echo ""
echo "==> Root preview"
echo "    timestamp:       $TS"
echo "    entries_count:   $ENTRY_COUNT"
echo "    entries_digest:  ${ENTRIES_DIGEST:0:16}…"

if [[ "$APPLY" -ne 1 ]]; then
  echo ""
  echo "(dry-run — pass --apply to sign and publish)"
  exit 0
fi

# --- Sign + upload -----------------------------------------------------
ROOT_FILE=$(mktemp)
SIG_FILE=$(mktemp)
trap 'cleanup; rm -f "$ROOT_FILE" "$SIG_FILE"' EXIT
printf '%s' "$ROOT_JSON" > "$ROOT_FILE"

"$OSSL" pkeyutl -sign -inkey "$RESOLVED_KEY" -rawin -in "$ROOT_FILE" -out "$SIG_FILE"

TS_SAFE="${TS//:/}"
ROOT_URI="${LOG_BUCKET}/roots/${TS_SAFE}-root.json"
SIG_URI="${ROOT_URI}.sig"

echo ""
echo "==> Publishing"
gcloud storage cp --content-type="application/json"          "$ROOT_FILE" "$ROOT_URI" 2>&1 | tail -1
gcloud storage cp --content-type="application/octet-stream"  "$SIG_FILE"  "$SIG_URI"  2>&1 | tail -1
# Also publish a stable "latest" pointer for easy lookup.
gcloud storage cp --content-type="application/json"          "$ROOT_FILE" "${LOG_BUCKET}/roots/latest-root.json"     2>&1 | tail -1
gcloud storage cp --content-type="application/octet-stream"  "$SIG_FILE"  "${LOG_BUCKET}/roots/latest-root.json.sig" 2>&1 | tail -1

echo ""
echo "Done."
echo "  root:    $ROOT_URI"
echo "  sig:     $SIG_URI"
echo "  latest:  ${LOG_BUCKET}/roots/latest-root.json"
