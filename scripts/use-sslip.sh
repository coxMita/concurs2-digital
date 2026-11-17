#!/usr/bin/env bash
# Sets DOMAIN in .env to <public-ip>.sslip.io (e.g. 203-0-113-7.sslip.io), so Caddy
# gets a real Let's Encrypt certificate without buying a domain.
#
#   ./scripts/use-sslip.sh            # detects this server's public IPv4
#   ./scripts/use-sslip.sh 1.2.3.4    # or give the IP explicitly
#
# Then: docker compose up -d   (or: docker compose up -d --force-recreate, if already running)
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f .env ] || cp .env.example .env

IP="${1:-}"
if [ -z "$IP" ]; then
  IP="$(curl -4 -fsS --max-time 10 https://api.ipify.org || true)"
fi
if ! [[ "$IP" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})$ ]]; then
  echo "Could not determine a public IPv4 address. Run: $0 <your-server-ip>" >&2
  exit 1
fi

DOMAIN="${IP//./-}.sslip.io"
if grep -q '^DOMAIN=' .env; then
  sed -i "s/^DOMAIN=.*/DOMAIN=${DOMAIN}/" .env
else
  echo "DOMAIN=${DOMAIN}" >> .env
fi

echo "DOMAIN=${DOMAIN} written to .env"
if command -v getent >/dev/null && ! getent hosts "$DOMAIN" >/dev/null; then
  echo "Warning: ${DOMAIN} does not resolve yet (check DNS/network)." >&2
fi
echo "Site address: https://${DOMAIN}"
