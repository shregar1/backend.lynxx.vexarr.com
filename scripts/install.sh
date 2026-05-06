#!/usr/bin/env bash
# Lynxx installer.
#
#   curl -fsSL https://api.lynxx.vexarr.com/install.sh | bash
#
# 1. Detects OS + architecture.
# 2. For each required binary, asks the licence/billing service for a
#    short-lived V4 signed URL (the binaries themselves live in a private
#    GCS bucket; only the billing service can mint download URLs).
# 3. Downloads each binary into /usr/local/bin (with sudo if needed)
#    or ~/.local/bin (fallback). After install, the user only needs
#    to set their API key and run `lynxx start`.

set -euo pipefail

# Where the API lives. Override with LYNXX_API_BASE for staging / self-host.
API_BASE="${LYNXX_API_BASE:-https://api.lynxx.vexarr.com}"
INSTALL_DIR="${LYNXX_INSTALL_DIR:-}"
BINARIES=("lynxx" "lynx-gateway")

# Older releases used LYNXX_RELEASE_BASE_URL — accept it as an explicit
# direct-download override, bypassing the signed-URL flow.
RELEASE_BASE_URL="${LYNXX_RELEASE_BASE_URL:-}"

# ---- coloured output (only on TTY) -----------------------------------------
if [ -t 1 ]; then
  C_RED='\033[31m'; C_GREEN='\033[32m'; C_YELLOW='\033[33m'; C_BOLD='\033[1m'; C_RESET='\033[0m'
else
  C_RED=''; C_GREEN=''; C_YELLOW=''; C_BOLD=''; C_RESET=''
fi

err()    { printf "${C_RED}error:${C_RESET} %s\n" "$*" >&2; exit 1; }
warn()   { printf "${C_YELLOW}warn:${C_RESET} %s\n"  "$*" >&2; }
info()   { printf "  %s\n" "$*"; }
title()  { printf "${C_BOLD}%s${C_RESET}\n" "$*"; }

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || err "$1 is required but not installed."
}
require_cmd curl

# ---- detect platform -------------------------------------------------------
detect_os() {
  case "$(uname -s)" in
    Linux*)  echo linux ;;
    Darwin*) echo darwin ;;
    *)       err "unsupported OS: $(uname -s). Lynxx supports linux and darwin." ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64)  echo amd64 ;;
    arm64|aarch64) echo arm64 ;;
    *)             err "unsupported architecture: $(uname -m). Lynxx supports amd64 and arm64." ;;
  esac
}

OS=$(detect_os)
ARCH=$(detect_arch)

# ---- pick install dir ------------------------------------------------------
choose_install_dir() {
  if [ -n "${INSTALL_DIR}" ]; then
    echo "${INSTALL_DIR}"
    return
  fi
  if [ -w "/usr/local/bin" ]; then
    echo "/usr/local/bin"
  elif command -v sudo >/dev/null 2>&1 && [ -d "/usr/local/bin" ]; then
    echo "/usr/local/bin"  # will sudo-install
  else
    mkdir -p "$HOME/.local/bin"
    echo "$HOME/.local/bin"
  fi
}

DEST_DIR=$(choose_install_dir)
USE_SUDO=""
if [ "${DEST_DIR}" = "/usr/local/bin" ] && [ ! -w "/usr/local/bin" ]; then
  if ! command -v sudo >/dev/null 2>&1; then
    err "no write access to /usr/local/bin and sudo is unavailable. Set LYNXX_INSTALL_DIR to override."
  fi
  USE_SUDO="sudo"
fi

# ---- already installed? ----------------------------------------------------
all_installed=1
for bin in "${BINARIES[@]}"; do
  if ! command -v "${bin}" >/dev/null 2>&1; then
    all_installed=0
    break
  fi
done
if [ "${all_installed}" -eq 1 ] && [ -z "${LYNXX_FORCE:-}" ]; then
  warn "lynxx and lynx-gateway already installed. Set LYNXX_FORCE=1 to reinstall."
  echo
  for bin in "${BINARIES[@]}"; do
    info "$(command -v "${bin}")"
  done
  exit 0
fi

# ---- resolve a download URL for one binary ---------------------------------
# If LYNXX_RELEASE_BASE_URL is set, use it as a direct prefix.
# Otherwise call the billing service for a short-lived signed URL.
resolve_url() {
  local bin="$1"
  if [ -n "${RELEASE_BASE_URL}" ]; then
    echo "${RELEASE_BASE_URL}/${OS}/${ARCH}/${bin}"
    return
  fi
  local resp
  resp=$(curl -fsSL --retry 3 \
    --get \
    --data-urlencode "name=${bin}" \
    --data-urlencode "os=${OS}" \
    --data-urlencode "arch=${ARCH}" \
    "${API_BASE}/v1/install/binary") || \
    err "failed to fetch signed URL for ${bin} from ${API_BASE}"
  # Parse the {"url":"..."} field without jq dep.
  local url
  url=$(printf '%s' "${resp}" | sed -n 's/.*"url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  [ -n "${url}" ] || err "could not parse signed URL from API response: ${resp}"
  echo "${url}"
}

# ---- download + install ----------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT

title "Installing Lynxx (${OS}/${ARCH}) → ${DEST_DIR}"
info "API: ${API_BASE}"

for bin in "${BINARIES[@]}"; do
  info "↓ ${bin}"
  url=$(resolve_url "${bin}")
  if ! curl -fsSL --retry 3 -o "${TMP}/${bin}" "${url}"; then
    err "download failed for ${bin}"
  fi
  chmod +x "${TMP}/${bin}"
done

for bin in "${BINARIES[@]}"; do
  ${USE_SUDO} install -m 0755 "${TMP}/${bin}" "${DEST_DIR}/${bin}"
done

# ---- PATH check ------------------------------------------------------------
if [[ ":$PATH:" != *":${DEST_DIR}:"* ]]; then
  warn "${DEST_DIR} is not on \$PATH."
  case "${SHELL:-}" in
    *zsh)  rc="$HOME/.zshrc" ;;
    *bash) rc="$HOME/.bashrc" ;;
    *)     rc="" ;;
  esac
  if [ -n "${rc}" ]; then
    info "add this to ${rc}:"
    info "  export PATH=\"${DEST_DIR}:\$PATH\""
  fi
fi

# ---- next steps ------------------------------------------------------------
echo
title "✓ Installed:"
for bin in "${BINARIES[@]}"; do
  info "${DEST_DIR}/${bin}"
done
echo
title "Next steps:"
info "1. Get an API key at https://lynxx.dev/account/api-keys"
info "2. Export it:    export LYNXX_API_KEY=lnx_live_..."
info "3. Start:        ${C_BOLD}lynxx start${C_RESET}"
echo
