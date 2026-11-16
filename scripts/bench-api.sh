#!/usr/bin/env bash
# Load test of the application API (`just bench-api`): builds bench/api in
# release mode, makes a fresh PostgreSQL database (docker compose, port 55439),
# seeds it, serves, and runs each scenario with `oha` (`brew install oha`).
#
#   scripts/bench-api.sh <label> [VAR=value ...]
#
# VAR=value pairs are passed to `serve` only, e.g. RUSTCMS_APP__MODE=api or
# RUSTCMS_QUEUE__ENABLED=false. BENCH_BIN uses a prebuilt binary (a baseline
# kept from an earlier commit); BENCH_SECS (15) and BENCH_CONNECTIONS (32) tune
# the load. Results: target/bench/<label>.md.
set -euo pipefail

label=${1:?usage: scripts/bench-api.sh <label> [VAR=value ...]}
shift
root=$(cd "$(dirname "$0")/.." && pwd)
secs=${BENCH_SECS:-15}
connections=${BENCH_CONNECTIONS:-32}
port=${BENCH_PORT:-8090}
base="http://127.0.0.1:$port"
out="$root/target/bench"
mkdir -p "$out"

command -v oha >/dev/null || { echo "oha is missing: brew install oha" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is missing" >&2; exit 1; }

bin=${BENCH_BIN:+$(cd "$(dirname "$BENCH_BIN")" && pwd)/$(basename "$BENCH_BIN")}
if [ -z "$bin" ]; then
    target=${CARGO_TARGET_DIR:-$root/target}
    (cd "$root/bench/api" && CARGO_TARGET_DIR="$target" cargo build --release -q)
    bin="$target/release/bench-api"
fi

cd "$root"
docker compose up -d --wait postgres >/dev/null
docker compose exec -T postgres psql -qU postgres -c "drop database if exists rustcms_bench" -c "create database rustcms_bench" >/dev/null

workdir=$(mktemp -d)
server=
cleanup() {
    # Only the server this script started (never `kill 0`: the whole group).
    if [ -n "$server" ]; then
        kill "$server" 2>/dev/null || true
        wait "$server" 2>/dev/null || true
    fi
    rm -rf "$workdir"
}
trap cleanup EXIT
cp -R "$root/bench/api/config" "$workdir/config"
cd "$workdir"
export APP_KEY=$("$bin" key:generate --show 2>/dev/null | tail -1)
export APP_ENV=local DATABASE_URL="postgres://postgres:postgres@localhost:55439/rustcms_bench"
export RUSTCMS_APP__LISTEN="127.0.0.1:$port" APP_URL="$base"
"$bin" migrate >/dev/null
token=$("$bin" bench:setup | tail -1)

env "$@" "$bin" serve >"$out/$label.server.log" 2>&1 &
server=$!
for _ in $(seq 1 100); do curl -sf "$base/health" >/dev/null && break; sleep 0.1; done
curl -sf "$base/health" >/dev/null || { echo "the server did not start: $out/$label.server.log" >&2; exit 1; }

jwt=$(curl -sf -X POST "$base/api/v1/auth/jwt" -H 'content-type: application/json' \
    -d '{"email":"bench@example.test","password":"bench password 123"}' | jq -r .accessToken)

# rss_kb cpu_seconds of the server. `ps -o time` is `[[dd-]hh:]mm:ss[.cc]`
# (macOS `m:ss.cc`, Linux `hh:mm:ss`).
usage() {
    ps -o rss=,time= -p "$server" | awk '{
        t = $2; days = 0
        if (index(t, "-")) { split(t, d, "-"); days = d[1]; t = d[2] }
        n = split(t, f, ":"); s = 0
        for (i = 1; i <= n; i++) s = s * 60 + f[i]
        printf "%d %.2f\n", $1, s + days * 86400 }'
}

{
    echo "# $label"
    echo
    echo "$(date -u +%FT%TZ), $(git -C "$root" rev-parse --short HEAD), ${secs}s × $connections connections, serve env: ${*:-none}"
    echo
    echo "| Scenario | req/s | p50 ms | p99 ms | non-2xx | CPU s | RSS MiB |"
    echo "|---|---:|---:|---:|---:|---:|---:|"
} >"$out/$label.md"

run() {
    local name=$1
    shift
    read -r _ cpu_before < <(usage)
    oha --no-tui --output-format json -z "${secs}s" -c "$connections" "$@" >"$out/$label.$name.json"
    read -r rss cpu_after < <(usage)
    jq -r --arg name "$name" --arg cpu "$(echo "$cpu_after - $cpu_before" | bc)" --arg rss "$((rss / 1024))" '
        (.statusCodeDistribution // {}) as $codes
        | ([$codes | to_entries[] | select(.key | test("^2") | not) | .value] | add // 0) as $bad
        | "| \($name) | \(.summary.requestsPerSec | floor) | \(.latencyPercentiles.p50 * 1000 * 100 | round / 100) | \(.latencyPercentiles.p99 * 1000 * 100 | round / 100) | \($bad) | \($cpu) | \($rss) |"
    ' "$out/$label.$name.json" >>"$out/$label.md"
}

run public "$base/api/v1/ping"
run pat_read -H "authorization: Bearer $token" "$base/api/v1/items"
[ -n "$jwt" ] && [ "$jwt" != null ] || { echo "no JWT" >&2; exit 1; }
run jwt_read -H "authorization: Bearer $jwt" "$base/api/v1/items"
run pat_write -m POST -H "authorization: Bearer $token" -H 'content-type: application/json' \
    -d '{"name":"bench"}' "$base/api/v1/items"
# An admin path answered by an API node (404) or the panel (its HTML shell).
run admin_path "$base/admin/"

cat "$out/$label.md"
