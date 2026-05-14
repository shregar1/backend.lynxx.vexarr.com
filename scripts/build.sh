#!/usr/bin/env bash
set -euo pipefail

# This script builds the Lynxx Gateway binary, optionally downloading
# the latest dashboard assets from a URL.
#
# Usage:
#   LYNXX_NODE_DASHBOARD_URL=https://... ./scripts/build.sh [cargo-args]

DASHBOARD_DIR="dashboard-dist"

if [[ -n "${LYNXX_NODE_DASHBOARD_URL:-}" ]]; then
    echo "==> Fetching dashboard from: ${LYNXX_NODE_DASHBOARD_URL}"
    
    # Create a clean dashboard directory
    rm -rf "${DASHBOARD_DIR}"
    mkdir -p "${DASHBOARD_DIR}"
    
    TMP_FILE=$(mktemp)
    if ! curl -fSL "${LYNXX_NODE_DASHBOARD_URL}" -o "${TMP_FILE}"; then
        echo "Error: Failed to download dashboard from ${LYNXX_NODE_DASHBOARD_URL}"
        rm -f "${TMP_FILE}"
        exit 1
    fi
    
    # Detect file type and extract
    # Use 'file' to determine type. On some systems mime-type might be different.
    TYPE=$(file "${TMP_FILE}")
    
    if echo "${TYPE}" | grep -q "Zip archive"; then
        echo "    Extracting ZIP..."
        unzip -q -o "${TMP_FILE}" -d "${DASHBOARD_DIR}"
    elif echo "${TYPE}" | grep -q "gzip compressed"; then
        echo "    Extracting Gzip Tarball..."
        tar -xzf "${TMP_FILE}" -C "${DASHBOARD_DIR}" --strip-components=1 2>/dev/null || tar -xzf "${TMP_FILE}" -C "${DASHBOARD_DIR}"
    else
        echo "    Unknown file type: ${TYPE}. Attempting raw tar extraction..."
        tar -xf "${TMP_FILE}" -C "${DASHBOARD_DIR}" --strip-components=1 2>/dev/null || tar -xf "${TMP_FILE}" -C "${DASHBOARD_DIR}" || {
            echo "Error: Could not extract dashboard. Unsupported format?"
            rm -f "${TMP_FILE}"
            exit 1
        }
    fi
    
    # Cleanup nested 'dist' folder if present (common in Vite exports)
    if [[ -d "${DASHBOARD_DIR}/dist" ]]; then
        echo "    Flattening 'dist/' directory..."
        mv "${DASHBOARD_DIR}/dist"/* "${DASHBOARD_DIR}/"
        rmdir "${DASHBOARD_DIR}/dist"
    fi
    
    rm -f "${TMP_FILE}"
    echo "==> Dashboard updated in ${DASHBOARD_DIR}/"
else
    if [[ ! -d "${DASHBOARD_DIR}" ]]; then
        echo "Error: ${DASHBOARD_DIR} missing and LYNXX_NODE_DASHBOARD_URL not set."
        exit 1
    fi
    echo "==> Using existing dashboard assets in ${DASHBOARD_DIR}/"
fi

echo "==> Building binaries..."
cargo build --release "$@"
