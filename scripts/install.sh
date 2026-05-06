#!/usr/bin/env bash
# Lynxx gateway installer.
#
#   curl -fsSL https://api.lynxx.vexarr.com/install.sh | bash
#
# Detects OS + architecture, downloads the matching release binaries from
# the public release bucket, drops them in /usr/local/bin (with sudo) or
# ~/.local/bin (without). After install, the user only needs to set their
# API key and run `lynxx start`.

set -euo pipefail

REPO_NAME="lynxx"
RELEASE_BASE_URL="${LYNXX_RELEASE_BASE_URL:-https://storage.googleapis.com/lynxx-releases/latest}"
INSTALL_DIR="${LYNXX_INSTALL_DIR:-}"
BINARIES=("lynxx" "lynx-gateway")

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

# ---- download + install ----------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT

title "Installing Lynxx (${OS}/${ARCH}) → ${DEST_DIR}"

for bin in "${BINARIES[@]}"; do
  url="${RELEASE_BASE_URL}/${OS}/${ARCH}/${bin}"
  info "↓ ${bin}"
  if ! curl -fsSL --retry 3 -o "${TMP}/${bin}" "${url}"; then
    err "download failed: ${url}\n       (set LYNXX_RELEASE_BASE_URL to a custom mirror)"
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
