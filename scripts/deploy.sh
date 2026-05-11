#!/usr/bin/env bash
set -euo pipefail

GCS_BUCKET="gs://vexarr-com/lynxx/binaries/linux/amd64"
TARGET="x86_64-unknown-linux-gnu"
VM_NAME="vexarr-com-micro"
VM_ZONE="us-central1-a"
BINARY_NAME="lynxx-gateway"

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
GIT_HASH=$(git rev-parse --short=8 HEAD 2>/dev/null || echo "unknown")
BUILD_TAG="v${VERSION}-${GIT_HASH}"
ARTIFACT="${BINARY_NAME}-${BUILD_TAG}"

echo "==> Building ${ARTIFACT} for ${TARGET}"
cargo zigbuild --release --target "${TARGET}" --bin "${BINARY_NAME}"

BINARY="target/${TARGET}/release/${BINARY_NAME}"
SIZE=$(du -h "${BINARY}" | cut -f1)
echo "==> Binary: ${SIZE}"

echo "==> Uploading ${ARTIFACT} to GCS"
gsutil cp "${BINARY}" "${GCS_BUCKET}/${ARTIFACT}"
gsutil cp "${BINARY}" "${GCS_BUCKET}/${BINARY_NAME}-latest"

echo "==> Listing recent versions in GCS"
gsutil ls -l "${GCS_BUCKET}/${BINARY_NAME}-v*" 2>/dev/null | tail -10

if [[ "${1:-}" == "--deploy" ]]; then
    echo "==> Zero-downtime deploy of ${BUILD_TAG} to ${VM_NAME}"
    gcloud compute ssh "${VM_NAME}" --zone="${VM_ZONE}" --command='
        set -e
        BUCKET="'"${GCS_BUCKET}"'"
        ARTIFACT="'"${ARTIFACT}"'"
        BIN="'"${BINARY_NAME}"'"

        echo "[1/7] Downloading new binary"
        sudo gsutil cp ${BUCKET}/${ARTIFACT} /tmp/${BIN}-new
        sudo chmod +x /tmp/${BIN}-new

        OLD_PID=$(systemctl show lynxx.service --property=MainPID --value)
        echo "[2/7] Old process PID: ${OLD_PID}"

        echo "[3/7] Starting new binary on staging ports (18443 admin, 18080 proxy)"
        sudo cp /etc/lynxx/config.toml /tmp/lynxx-staging.toml
        sudo sed -i "s/^admin_port.*/admin_port = 18443/" /tmp/lynxx-staging.toml
        sudo sed -i "s/^proxy_port.*/proxy_port = 18080/" /tmp/lynxx-staging.toml

        LYNXX_API_KEY=$(grep "LYNXX_API_KEY=" /etc/systemd/system/lynxx.service | sed "s/.*LYNXX_API_KEY=//")
        LYNXX_LICENSE_VERIFY_URL=$(grep "LYNXX_LICENSE_VERIFY_URL=" /etc/systemd/system/lynxx.service | sed "s/.*LYNXX_LICENSE_VERIFY_URL=//" || true)

        sudo LYNXX_API_KEY="${LYNXX_API_KEY}" \
             LYNXX_LICENSE_VERIFY_URL="${LYNXX_LICENSE_VERIFY_URL}" \
             HOME=/var/lib/lynxx \
             /tmp/${BIN}-new --config /tmp/lynxx-staging.toml &
        STAGING_PID=$!
        sleep 3

        echo "[4/7] Health-checking staging instance"
        if curl -sf http://localhost:18443/healthz > /dev/null 2>&1; then
            echo "    Staging healthy: $(curl -s http://localhost:18443/healthz)"
        else
            echo "    FATAL: staging instance failed health check, aborting"
            sudo kill ${STAGING_PID} 2>/dev/null || true
            exit 1
        fi

        echo "[5/7] Redirecting traffic via iptables (443→18080, 9001→18443)"
        sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 18080
        sudo iptables -t nat -A OUTPUT -p tcp --dport 443 -o lo -j REDIRECT --to-port 18080
        sudo iptables -t nat -A PREROUTING -p tcp --dport 9001 -j REDIRECT --to-port 18443
        sudo iptables -t nat -A OUTPUT -p tcp --dport 9001 -o lo -j REDIRECT --to-port 18443
        sudo iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo iptables -t nat -A OUTPUT -p tcp --dport 80 -o lo -j REDIRECT --to-port 18080 2>/dev/null || true

        echo "    Traffic redirected — staging serving, old process draining"
        sudo systemctl stop lynxx.service

        echo "[6/7] Swapping binary and starting on real ports"
        sudo cp /usr/local/bin/${BIN} /usr/local/bin/${BIN}.bak
        sudo mv /tmp/${BIN}-new /usr/local/bin/${BIN}
        sudo systemctl start lynxx.service

        # Wait for the new systemd process to be healthy before cutting over
        echo "    Waiting for new process to be ready..."
        for i in $(seq 1 20); do
            if curl -sf http://localhost:18443/healthz > /dev/null 2>&1; then
                # Port 18443 still redirects to staging — check the real port
                # by talking directly to the new PID via /proc
                NEW_PID=$(systemctl show lynxx.service --property=MainPID --value)
                if [ "${NEW_PID}" != "0" ] && [ -d "/proc/${NEW_PID}" ]; then
                    break
                fi
            fi
            sleep 0.5
        done

        echo "[7/7] Cutting over: removing iptables, stopping staging"
        sudo iptables -t nat -D PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo iptables -t nat -D OUTPUT -p tcp --dport 443 -o lo -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo iptables -t nat -D PREROUTING -p tcp --dport 9001 -j REDIRECT --to-port 18443 2>/dev/null || true
        sudo iptables -t nat -D OUTPUT -p tcp --dport 9001 -o lo -j REDIRECT --to-port 18443 2>/dev/null || true
        sudo iptables -t nat -D PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo iptables -t nat -D OUTPUT -p tcp --dport 80 -o lo -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo kill ${STAGING_PID} 2>/dev/null || true

        sleep 1
        echo "--- Status ---"
        curl -s http://localhost:9001/healthz
    '
    echo ""
    echo "==> Deployed ${BUILD_TAG} (zero-downtime)"
elif [[ "${1:-}" == "--rollback" ]]; then
    ROLLBACK_TO="${2:-}"
    if [[ -z "${ROLLBACK_TO}" ]]; then
        echo "Usage: ./scripts/deploy.sh --rollback <build-tag>"
        echo "Available versions:"
        gsutil ls "${GCS_BUCKET}/${BINARY_NAME}-v*" 2>/dev/null | sed "s|${GCS_BUCKET}/${BINARY_NAME}-||"
        exit 1
    fi
    echo "==> Rolling back to ${ROLLBACK_TO}"
    gcloud compute ssh "${VM_NAME}" --zone="${VM_ZONE}" --command='
        set -e
        BUCKET="'"${GCS_BUCKET}"'"
        BIN="'"${BINARY_NAME}"'"
        TAG="'"${ROLLBACK_TO}"'"

        sudo gsutil cp ${BUCKET}/${BIN}-${TAG} /tmp/${BIN}-rollback
        sudo chmod +x /tmp/${BIN}-rollback

        sudo cp /etc/lynxx/config.toml /tmp/lynxx-staging.toml
        sudo sed -i "s/^admin_port.*/admin_port = 18443/" /tmp/lynxx-staging.toml
        sudo sed -i "s/^proxy_port.*/proxy_port = 18080/" /tmp/lynxx-staging.toml

        LYNXX_API_KEY=$(grep "LYNXX_API_KEY=" /etc/systemd/system/lynxx.service | sed "s/.*LYNXX_API_KEY=//")
        LYNXX_LICENSE_VERIFY_URL=$(grep "LYNXX_LICENSE_VERIFY_URL=" /etc/systemd/system/lynxx.service | sed "s/.*LYNXX_LICENSE_VERIFY_URL=//" || true)

        sudo LYNXX_API_KEY="${LYNXX_API_KEY}" \
             LYNXX_LICENSE_VERIFY_URL="${LYNXX_LICENSE_VERIFY_URL}" \
             HOME=/var/lib/lynxx \
             /tmp/${BIN}-rollback --config /tmp/lynxx-staging.toml &
        STAGING_PID=$!
        sleep 3

        if ! curl -sf http://localhost:18443/healthz > /dev/null 2>&1; then
            echo "FATAL: rollback binary failed health check"
            sudo kill ${STAGING_PID} 2>/dev/null || true
            exit 1
        fi

        sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 18080
        sudo iptables -t nat -A OUTPUT -p tcp --dport 443 -o lo -j REDIRECT --to-port 18080
        sudo iptables -t nat -A PREROUTING -p tcp --dport 9001 -j REDIRECT --to-port 18443
        sudo iptables -t nat -A OUTPUT -p tcp --dport 9001 -o lo -j REDIRECT --to-port 18443

        sudo systemctl stop lynxx.service

        sudo cp /usr/local/bin/${BIN} /usr/local/bin/${BIN}.bak
        sudo mv /tmp/${BIN}-rollback /usr/local/bin/${BIN}
        sudo systemctl start lynxx.service

        for i in $(seq 1 20); do
            NEW_PID=$(systemctl show lynxx.service --property=MainPID --value)
            if [ "${NEW_PID}" != "0" ] && [ -d "/proc/${NEW_PID}" ]; then
                break
            fi
            sleep 0.5
        done

        sudo iptables -t nat -D PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo iptables -t nat -D OUTPUT -p tcp --dport 443 -o lo -j REDIRECT --to-port 18080 2>/dev/null || true
        sudo iptables -t nat -D PREROUTING -p tcp --dport 9001 -j REDIRECT --to-port 18443 2>/dev/null || true
        sudo iptables -t nat -D OUTPUT -p tcp --dport 9001 -o lo -j REDIRECT --to-port 18443 2>/dev/null || true
        sudo kill ${STAGING_PID} 2>/dev/null || true

        sleep 1
        curl -s http://localhost:9001/healthz
    '
    echo ""
    echo "==> Rolled back to ${ROLLBACK_TO} (zero-downtime)"
else
    echo ""
    echo "Binary uploaded. To deploy to VM:"
    echo "  ./scripts/deploy.sh --deploy"
    echo ""
    echo "To rollback:"
    echo "  ./scripts/deploy.sh --rollback <build-tag>"
fi
