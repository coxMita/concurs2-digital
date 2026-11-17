#!/bin/sh
# Runs in the one-shot `init` service before the database and the app start.
# Fills empty POSTGRES_PASSWORD and SETUP_TOKEN in .env with random values.
# Existing values are never changed (the database keeps the password it was
# initialized with).
set -eu
ENV_FILE=/work/.env

if [ ! -f "$ENV_FILE" ]; then
  echo "init: .env not found. Run: cp .env.example .env" >&2
  exit 1
fi

gen() { tr -dc 'A-Za-z0-9' < /dev/urandom | head -c "$1"; }

current() { sed -n "s/^$1=//p" "$ENV_FILE" | tail -n 1 | tr -d '\r'; }

set_value() {
  key="$1"; value="$2"; tmp="$(mktemp)"
  if grep -q "^$key=" "$ENV_FILE"; then
    awk -v k="$key" -v v="$value" 'BEGIN{FS=OFS="="} $1==k{print k"="v; next} {print}' "$ENV_FILE" > "$tmp"
  else
    cat "$ENV_FILE" > "$tmp"
    printf '%s=%s\n' "$key" "$value" >> "$tmp"
  fi
  # Write in place: .env is a single-file bind mount, so it cannot be replaced.
  cat "$tmp" > "$ENV_FILE"
  rm -f "$tmp"
}

if [ -z "$(current POSTGRES_PASSWORD)" ]; then
  set_value POSTGRES_PASSWORD "$(gen 32)"
  echo "init: generated POSTGRES_PASSWORD in .env"
fi
if [ -z "$(current SETUP_TOKEN)" ]; then
  set_value SETUP_TOKEN "$(gen 24)"
  echo "init: generated SETUP_TOKEN in .env"
fi
echo "init: secrets ready"
