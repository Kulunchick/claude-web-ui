#!/usr/bin/env bash
# Installs the latest Gateway build from the rolling GitHub release when it
# differs from the installed one, restarts the service, and rolls back if the
# new build does not answer its health check.
set -euo pipefail

REPO="${CLAUDE_WEB_UI_REPO:-Kulunchick/claude-web-ui}"
PREFIX="${CLAUDE_WEB_UI_PREFIX:-/opt/claude-web-ui}"
SERVICE="${CLAUDE_WEB_UI_SERVICE:-claude-web-ui}"
HEALTH_URL="${CLAUDE_WEB_UI_HEALTH_URL:-http://127.0.0.1:8080/api/auth/status}"
ASSET="claude-web-ui-server-linux-x86_64"
BASE="https://github.com/${REPO}/releases/download/gateway-latest"

BIN="${PREFIX}/bin/claude-web-ui-server"
VERSION_FILE="${PREFIX}/version"
FAILED_FILE="${PREFIX}/failed-version"

exec 9>"${PREFIX}/update.lock"
flock -n 9 || exit 0

remote="$(curl -fsSL --retry 3 "${BASE}/version.txt" | tr -d '[:space:]')"
[[ "${remote}" =~ ^[0-9a-f]{40}$ ]] || { echo "unexpected version: ${remote}" >&2; exit 1; }
current="$(cat "${VERSION_FILE}" 2>/dev/null || true)"
if [[ "${remote}" == "${current}" && -x "${BIN}" ]]; then
  exit 0
fi
# Do not retry a build that already failed its health check.
if [[ "${remote}" == "$(cat "${FAILED_FILE}" 2>/dev/null || true)" ]]; then
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
curl -fsSL --retry 3 -o "${tmp}/${ASSET}" "${BASE}/${ASSET}"
curl -fsSL --retry 3 -o "${tmp}/${ASSET}.sha256" "${BASE}/${ASSET}.sha256"
(cd "${tmp}" && sha256sum --check --quiet "${ASSET}.sha256")
chmod 0755 "${tmp}/${ASSET}"

echo "updating ${current:-none} -> ${remote}"
mkdir -p "${PREFIX}/bin"
if [[ -x "${BIN}" ]]; then
  cp -p "${BIN}" "${BIN}.prev"
fi
mv -f "${tmp}/${ASSET}" "${BIN}.new"
mv -f "${BIN}.new" "${BIN}"
systemctl restart "${SERVICE}"

for _ in $(seq 1 30); do
  if curl -fsS -o /dev/null "${HEALTH_URL}"; then
    echo "${remote}" > "${VERSION_FILE}"
    echo "running ${remote}"
    exit 0
  fi
  sleep 1
done

echo "health check failed for ${remote}" >&2
echo "${remote}" > "${FAILED_FILE}"
if [[ -x "${BIN}.prev" ]]; then
  mv -f "${BIN}.prev" "${BIN}"
  systemctl restart "${SERVICE}"
  echo "rolled back to ${current}" >&2
fi
exit 1
