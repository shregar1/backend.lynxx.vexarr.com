#!/usr/bin/env bash
# Lynxx installer.
#
#   curl -fsSL https://get.lynxx.vexarr.com | sh
#
# 1. Detects OS + architecture.
# 2. Streams each binary through api.lynxx.vexarr.com (which reads
#    them from the private release bucket as the gateway service
#    account). Customers never talk to a storage hostname directly.
# 3. Installs into /usr/local/bin if writable / sudo runs without a
#    prompt, otherwise ~/.local/bin. After install, set your API key
#    (LYNXX_API_KEY) and run `lynxx start` — the binary refuses to
#    run without a valid key tied to an active subscription.

set -euo pipefail

# Where the API lives. Both the binary download (/v1/install/binary)
# and the licence verifier (/v1/license/verify) live here. Override
# with LYNXX_API_BASE for staging / self-host.
API_BASE="${LYNXX_API_BASE:-https://api.lynxx.vexarr.com}"
INSTALL_DIR="${LYNXX_INSTALL_DIR:-}"
BINARIES=("lynxx" "lynxx-gateway")

# Air-gapped / mirror install: set LYNXX_RELEASE_BASE_URL to a static
# host (e.g. an internal CDN) holding the same os/arch/binary layout
# and the script will pull from there instead of the API.
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
# Prefer /usr/local/bin when we can write to it directly, OR when we
# can sudo without prompting (passwordless / cached creds). When piped
# from `curl ... | sh` there's no tty, so a sudo password prompt would
# wedge — fall back to ~/.local/bin in that case.
sudo_noninteractive_ok() {
  command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1
}

choose_install_dir() {
  if [ -n "${INSTALL_DIR}" ]; then
    echo "${INSTALL_DIR}"
    return
  fi
  if [ -w "/usr/local/bin" ]; then
    echo "/usr/local/bin"
  elif [ -d "/usr/local/bin" ] && sudo_noninteractive_ok; then
    echo "/usr/local/bin"  # will sudo-install non-interactively
  else
    mkdir -p "$HOME/.local/bin"
    echo "$HOME/.local/bin"
  fi
}

DEST_DIR=$(choose_install_dir)
USE_SUDO=""
if [ "${DEST_DIR}" = "/usr/local/bin" ] && [ ! -w "/usr/local/bin" ]; then
  if sudo_noninteractive_ok; then
    USE_SUDO="sudo -n"
  else
    err "no write access to /usr/local/bin and sudo isn't available without a prompt. Set LYNXX_INSTALL_DIR to override."
  fi
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
  warn "lynxx and lynxx-gateway already installed. Set LYNXX_FORCE=1 to reinstall."
  echo
  for bin in "${BINARIES[@]}"; do
    info "$(command -v "${bin}")"
  done
  exit 0
fi

# ---- pick a download URL ---------------------------------------------------
# Default: stream through the API so every byte flows over the api
# domain (single hostname, no third-party storage host visible to the
# customer). LYNXX_RELEASE_BASE_URL overrides for self-host / mirror.
# License server still gates runtime — a downloaded binary won't
# actually run without a valid LYNXX_API_KEY tied to an active sub.
download_url() {
  local bin="$1"
  if [ -n "${RELEASE_BASE_URL}" ]; then
    echo "${RELEASE_BASE_URL}/${OS}/${ARCH}/${bin}"
  else
    echo "${API_BASE}/v1/install/binary?name=${bin}&os=${OS}&arch=${ARCH}"
  fi
}

# ---- download + install ----------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT

title "Installing Lynxx (${OS}/${ARCH}) → ${DEST_DIR}"
info "API: ${API_BASE}"

for bin in "${BINARIES[@]}"; do
  info "↓ ${bin}"
  url=$(download_url "${bin}")
  if ! curl -fL --retry 3 -o "${TMP}/${bin}" "${url}"; then
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
info "1. Get an API key at https://lynxx.vexarr.com/account/api-keys"
info "2. Export it:    export LYNXX_API_KEY=lnx_live_..."
info "3. Start:        ${C_BOLD}lynxx start${C_RESET}"
echo
