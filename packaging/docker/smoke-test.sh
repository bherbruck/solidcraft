#!/usr/bin/env bash
# Start the web app image and check that it serves the app the way browsers need it.
#
# Usage: packaging/docker/smoke-test.sh [IMAGE]     (default: solidcraft-web:test)
#
# Runs the container read-only (as compose.yaml does) and checks the page, the service worker
# and the wasm: their status, types and caching (nginx.conf).
set -euo pipefail

IMAGE="${1:-solidcraft-web:test}"
NAME="solidcraft-smoke-$$"
docker run -d --rm --name "$NAME" --read-only --tmpfs /tmp --cap-drop ALL -p 127.0.0.1::8080 "$IMAGE" >/dev/null
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
PORT="$(docker port "$NAME" 8080/tcp | head -n1 | sed 's/.*://')"
BASE="http://127.0.0.1:$PORT"

for _ in $(seq 1 30); do
  curl -fs -o /dev/null "$BASE/" && break
  sleep 1
done

fail() { echo "error: $*" >&2; docker logs "$NAME" >&2 || true; exit 1; }

# header URL NAME: the value of response header NAME for URL ("" when missing).
header() { curl -fsI "$1" | tr -d '\r' | awk -v h="$(echo "$2" | tr '[:upper:]' '[:lower:]')" -F': ' 'tolower($1) == h { print $2 }'; }

curl -fs "$BASE/" | grep -q 'solidcraft_canvas' || fail "index.html is not the app"
[ "$(header "$BASE/" Cache-Control)" = no-cache ] || fail "index.html must not be cached"
[ "$(header "$BASE/sw.js" Cache-Control)" = no-cache ] || fail "sw.js must not be cached"
curl -fs "$BASE/sw.js" | grep -q 'const VERSION = "dev"' && fail "sw.js is the unbuilt template (not a release build)"
[ "$(header "$BASE/build.json" Cache-Control)" = no-cache ] || fail "build.json must not be cached"

WASM="$(curl -fs "$BASE/" | grep -o 'solidcraft-web-[0-9a-f]*_bg\.wasm' | head -n1)"
[ -n "$WASM" ] || fail "index.html names no wasm"
[ "$(header "$BASE/$WASM" Content-Type)" = application/wasm ] || fail "$WASM is not served as application/wasm"
header "$BASE/$WASM" Cache-Control | grep -q immutable || fail "$WASM should be cached as immutable"
[ "$(header "$BASE/manifest.webmanifest" Content-Type)" = application/manifest+json ] || fail "wrong manifest type"
code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/no-such-file")"
[ "$code" = 404 ] || fail "a missing file answers $code, not 404"

echo "smoke test passed: $IMAGE serves $WASM"
