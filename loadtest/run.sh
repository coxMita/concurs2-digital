#!/usr/bin/env bash
# Runs the k6 load test through Docker (no local k6 install needed).
#
#   ADMIN_USER=prof ADMIN_PASS=... ./loadtest/run.sh
#
# Options (environment variables):
#   BASE_URL        default https://localhost (the Caddy front end)
#   STUDENTS        default 500
#   QUESTIONS       default 20
#   LOGIN_WINDOW_S  default 60   (all students log in within this window)
#   PAUSE_MIN_S / PAUSE_MAX_S    default 3 / 8 seconds per question
#
# The test creates its own exam ("Test de încărcare ...") with its own accounts;
# delete it from the admin panel afterwards.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p results
: "${ADMIN_USER:?set ADMIN_USER (an admin account created via /setup)}"
: "${ADMIN_PASS:?set ADMIN_PASS}"
exec docker run --rm -i --network host \
  -v "$PWD:/scripts" \
  -e BASE_URL="${BASE_URL:-https://localhost}" \
  -e STUDENTS="${STUDENTS:-500}" \
  -e QUESTIONS="${QUESTIONS:-20}" \
  -e LOGIN_WINDOW_S="${LOGIN_WINDOW_S:-60}" \
  -e PAUSE_MIN_S="${PAUSE_MIN_S:-3}" \
  -e PAUSE_MAX_S="${PAUSE_MAX_S:-8}" \
  -e ADMIN_USER -e ADMIN_PASS \
  -u "$(id -u):$(id -g)" \
  grafana/k6:0.54.0 run /scripts/k6.js
