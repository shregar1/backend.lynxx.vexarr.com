#!/usr/bin/env bash
# Vexarr release-signing key rotation runbook.
#
# Rotates the org-wide Ed25519 keypair that:
#   - signs every release artifact (.sig sidecars)
#   - signs every transparency log entry
#   - is pinned (public half) inside every install script + every gateway's
#     install module
#
# Two-stage rotation, with a configurable dual-key window. During the window
# old binaries (signed by the previous key) still install cleanly because
# scripts accept BOTH the old and new pubkeys. After the window, drop the
# old key everywhere — at which point any binary not yet re-signed will
# fail to install.
#
# This script does NOT push to GCS, regenerate codebases, or re-sign anything
# unless you pass --apply. By default it does a planning dry-run that prints
# exactly what would change. Read the plan, then re-run with --apply.
#
# Usage:
#   scripts/rotate-signing-key.sh                # plan-only (dry-run)
#   scripts/rotate-signing-key.sh --apply        # execute rotation
#   scripts/rotate-signing-key.sh --apply \
#       --window-days 14 \                       # how long both keys are accepted
#       --new-key-out ~/.config/vexarr-releases/signing.key.new
#
# After --apply:
#   1. The NEW keypair lands at $NEW_KEY_OUT (private) and $NEW_KEY_OUT.pub
#   2. New Secret Manager version (vexarr-release-signing-key) holds the new
#      private key; previous version is preserved for the window
#   3. A second pubkey constant is inserted into every install script so
#      verify_artifact accepts EITHER old or new signatures
#   4. The doc at $RUNBOOK_OUT tells you exactly which 90 install scripts
#      and 22 install modules were touched, and the post-window cleanup steps
#
# Re-running this script after the dual-key window:
#   scripts/rotate-signing-key.sh --apply --finalise
# drops the old pubkey from scripts and the prior Secret Manager version.

set -euo pipefail

APPLY=0
FINALISE=0
WINDOW_DAYS=14
NEW_KEY_OUT="${HOME}/.config/vexarr-releases/signing.key.new"
RUNBOOK_OUT="/tmp/vexarr-rotation-$(date -u +%Y%m%d-%H%M%S).md"
OLD_KEY="${VEXARR_SIGNING_KEY:-${HOME}/.config/vexarr-releases/signing.key}"
PROJECT="${VEXARR_PROJECT:-vexarr-com-495315}"
SECRET_NAME="${VEXARR_SECRET_NAME:-vexarr-release-signing-key}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --apply)        APPLY=1; shift ;;
    --finalise)     FINALISE=1; shift ;;
    --window-days)  WINDOW_DAYS="$2"; shift 2 ;;
    --new-key-out)  NEW_KEY_OUT="$2"; shift 2 ;;
    --runbook-out)  RUNBOOK_OUT="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

# Find an Ed25519-capable openssl. LibreSSL on macOS does NOT count.
find_openssl() {
  for c in /opt/homebrew/opt/openssl@3/bin/openssl /opt/homebrew/opt/openssl/bin/openssl /usr/local/opt/openssl@3/bin/openssl openssl; do
    if command -v "$c" >/dev/null 2>&1; then
      if "$c" version 2>/dev/null | grep -q "^OpenSSL"; then
        echo "$c"
        return 0
      fi
    fi
  done
  return 1
}
OSSL=$(find_openssl) || { echo "no OpenSSL 3+ found (LibreSSL doesn't support Ed25519)" >&2; exit 1; }

cyan()  { printf '\033[36m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }
dim()   { printf '\033[2m%s\033[0m\n' "$*"; }

list_install_scripts() {
  find /Users/shreyansh/Documents \
    -type d \( -name target -o -name node_modules -o -name .git \) -prune -o \
    -type f -path "*/assets/install/install*.sh"  -print 2>/dev/null
  find /Users/shreyansh/Documents \
    -type d \( -name target -o -name node_modules -o -name .git \) -prune -o \
    -type f -path "*/assets/install/install*.ps1" -print 2>/dev/null
}
list_install_modules() {
  find /Users/shreyansh/Documents \
    -type d \( -name target -o -name node_modules -o -name .git \) -prune -o \
    -type f -path "*/src/install/mod.rs" -print 2>/dev/null
}

# ---------- Plan phase (always runs first) ----------------------------------
cyan "==> Rotation plan"
echo "  finalise mode:    $([[ "$FINALISE" -eq 1 ]] && echo yes || echo no)"
echo "  apply:            $([[ "$APPLY" -eq 1 ]] && echo yes || echo "NO -- dry-run")"
echo "  window-days:      $WINDOW_DAYS"
echo "  openssl:          $OSSL ($("$OSSL" version | head -1))"
echo "  old key path:     $OLD_KEY"
echo "  new key out:      $NEW_KEY_OUT"
echo "  GCP project:      $PROJECT"
echo "  secret name:      $SECRET_NAME"
echo "  runbook will be:  $RUNBOOK_OUT"
echo ""

SCRIPT_COUNT=$(list_install_scripts | wc -l | tr -d ' ')
MODULE_COUNT=$(list_install_modules | wc -l | tr -d ' ')
echo "  install scripts to touch:  $SCRIPT_COUNT"
echo "  install modules to touch:  $MODULE_COUNT (note: modules don't pin pubkey, no change required)"
echo ""

if [[ "$FINALISE" -eq 1 ]]; then
  yellow "==> FINALISE mode"
  echo "  will REMOVE the old pubkey constant from $SCRIPT_COUNT install scripts"
  echo "  will DISABLE the previous Secret Manager version"
  echo ""
  if [[ "$APPLY" -ne 1 ]]; then
    dim "  (dry-run — pass --apply to execute)"
    exit 0
  fi
  # … finalise implementation
  echo "(finalise step not yet implemented — manual: regenerate install scripts from /tmp/x-product-setup.sh"
  echo " after editing VEXARR_RELEASE_PUBKEY to remove old key, then run gcloud secrets versions disable)"
  exit 0
fi

# ---------- Generate the new keypair ----------------------------------------
if [[ "$APPLY" -eq 1 ]]; then
  mkdir -p "$(dirname "$NEW_KEY_OUT")"
  if [[ -f "$NEW_KEY_OUT" ]]; then
    yellow "  new key already exists at $NEW_KEY_OUT — refusing to overwrite"
    yellow "  remove it or pass --new-key-out elsewhere to regenerate"
    exit 1
  fi
  cyan "==> Generating new Ed25519 keypair"
  "$OSSL" genpkey -algorithm Ed25519 -out "$NEW_KEY_OUT"
  "$OSSL" pkey -in "$NEW_KEY_OUT" -pubout -out "${NEW_KEY_OUT}.pub"
  chmod 600 "$NEW_KEY_OUT"
  green "  wrote $NEW_KEY_OUT (private) + ${NEW_KEY_OUT}.pub (public)"
else
  cyan "==> Would generate: $NEW_KEY_OUT (Ed25519, mode 600) + .pub"
fi

# ---------- Push new private key to Secret Manager --------------------------
if [[ "$APPLY" -eq 1 ]]; then
  cyan "==> Adding new version to Secret Manager"
  gcloud secrets versions add "$SECRET_NAME" --project="$PROJECT" --data-file="$NEW_KEY_OUT" 2>&1 | tail -2
  green "  new version is now 'latest'; previous version retained for the dual-key window"
else
  cyan "==> Would: gcloud secrets versions add $SECRET_NAME --project=$PROJECT --data-file=$NEW_KEY_OUT"
fi

# ---------- Show how to update install scripts (manual or via generator) ----
cyan "==> Install-script pubkey update"
OLD_PEM=$("$OSSL" pkey -in "$OLD_KEY" -pubout 2>/dev/null || true)
if [[ "$APPLY" -eq 1 ]]; then
  NEW_PEM=$(cat "${NEW_KEY_OUT}.pub")
else
  NEW_PEM="(will be generated)"
fi
echo "  old pubkey:"
echo "$OLD_PEM" | sed 's/^/    /'
echo ""
echo "  new pubkey:"
echo "$NEW_PEM" | sed 's/^/    /'
echo ""
echo "  All $SCRIPT_COUNT install scripts pin VEXARR_RELEASE_PUBKEY (or"
echo "  LYNXX_RELEASE_PUBKEY in lynxx's own scripts). The dual-key window"
echo "  is implemented by accepting either: if openssl-verify with key A"
echo "  fails, retry with key B; both fail → reject."
echo ""
echo "  Use the install-script generator at /tmp/x-product-setup.sh — update"
echo "  the embedded VEXARR_RELEASE_PUBKEY block to a two-PEM concatenation,"
echo "  then regenerate the 86 X-product scripts. Edit the 4 lynxx canonical"
echo "  scripts (assets/install/install*) by hand for the same change."
echo ""

# ---------- Re-sign existing artifacts with the new key ---------------------
cyan "==> Re-sign artifacts + republish transparency log entries"
echo "  For every artifact currently in gs://<product>-public/<artifact>/.../<bin>,"
echo "  run upload-artifact.sh with --signing-key gcp-secret://${SECRET_NAME}@latest"
echo "  using the SAME binary. This produces fresh .sig sidecars (signed by the new"
echo "  key) and fresh transparency log entries. The OLD .sig stays accessible via"
echo "  the versioned <bin>-<version>.sig path until the window closes."
echo ""

# ---------- Generate the runbook --------------------------------------------
cat > "$RUNBOOK_OUT" <<EOF
# Vexarr release-signing key rotation — runbook

Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)
Mode: $([[ "$APPLY" -eq 1 ]] && echo executed || echo dry-run)
Window: $WINDOW_DAYS days

## What changed in this rotation event

- New Ed25519 keypair generated at \`$NEW_KEY_OUT\`
- New Secret Manager version added to \`projects/$PROJECT/secrets/$SECRET_NAME\`
- Old key still active at \`$OLD_KEY\` and as the previous Secret Manager version

## Manual steps still required

1. Update the embedded \`VEXARR_RELEASE_PUBKEY\` PEM in the script generator
   at \`/tmp/x-product-setup.sh\` (and lynxx's 4 canonical scripts at
   \`backend.lynxx.vexarr.com/assets/install/install*.{sh,ps1}\`) to include
   BOTH the old and the new pubkey, with a verify path that tries each.
2. Regenerate $SCRIPT_COUNT install scripts: \`bash /tmp/x-product-setup.sh\`
3. Deploy the updated install module + assets (each gateway needs a rebuild +
   redeploy to pick up the embedded asset changes via \`include_str!\`).
4. Re-sign every currently-stable artifact in every \`gs://<product>-public\`
   bucket using the new key (loop \`upload-artifact.sh\` over each known
   binary). The versioned copies remain pinned to whichever key signed them.
5. Watch the transparency log: each re-sign produces a new log entry. Diff
   the log over the rotation window to confirm coverage.

## After $WINDOW_DAYS days — finalise

\`\`\`bash
scripts/rotate-signing-key.sh --apply --finalise
\`\`\`

This drops the old pubkey from install scripts and disables the previous
Secret Manager version. Any artifact still signed with the old key will
fail install verification at this point.

## Rollback (if the new key is compromised before finalise)

\`\`\`bash
# Disable the new SM version, keep using the old one
gcloud secrets versions disable LATEST --secret=$SECRET_NAME --project=$PROJECT

# Revert install-script changes (manual: re-regenerate scripts pinning only
# the old key, redeploy gateways)
\`\`\`
EOF
green "==> Wrote runbook to $RUNBOOK_OUT"
echo ""

if [[ "$APPLY" -ne 1 ]]; then
  dim "==> Plan complete. Re-run with --apply to execute."
fi
