#!/usr/bin/env bash
# Rejoue le job `check` de .github/workflows/ci.yml (garder les deux synchronisés).
set -euo pipefail

cd "$(dirname "$0")/.."

# Comme la CI : les macros sqlx compilent contre .sqlx/, jamais contre husker.db.
export SQLX_OFFLINE=true

step() {
    local name=$1
    shift
    echo "▶ $name"
    if ! "$@"; then
        echo "✖ ÉCHEC : $name" >&2
        exit 1
    fi
}

step "cargo fmt --check" cargo fmt --check
step "cargo clippy -D warnings" cargo clippy --all-targets -- -D warnings
# --test-threads=1 : mêmes env vars globales partagées que dans la CI (voir ci.yml).
step "cargo test (offline)" cargo test -- --test-threads=1

echo "✔ CI locale OK"
