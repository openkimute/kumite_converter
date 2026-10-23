#!/bin/sh
# The checks: `sh check.sh [commit|push|fmt|lint|test|deny]`.
#   commit (the pre-commit hook): rustfmt; clippy with warnings as errors; the tests (they compare with the disc when
#                                 T3_BIN is set);
#   push (the pre-push hook):     those, and cargo deny (advisories, licences, sources, bans: deny.toml).
set -e
cd "$(dirname "$0")"
step() { printf '\n== %s\n' "$*"; }
fmt() {
    step fmt
    cargo fmt --all --check
}
lint() {
    step clippy
    cargo clippy --workspace --all-targets --release -q -- -D warnings
}
tests() {
    step test
    if ! out=$(cargo test --workspace --release 2>&1); then
        printf '%s\n' "$out" | grep -vE '^test .* ok$|^running |^$' | tail -80
        return 1
    fi
    printf '%s\n' "$out" | awk '/^test result/ { p += $4; i += $8 } END { printf "%d passed, %d ignored\n", p, i }'
}
deny() {
    step deny
    cargo deny --log-level error check
}
case "${1:-push}" in
    commit) fmt; lint; tests ;;
    push) fmt; lint; tests; deny ;;
    fmt) fmt ;;
    lint) lint ;;
    test) tests ;;
    deny) deny ;;
    *) echo "usage: sh check.sh [commit|push|fmt|lint|test|deny]"; exit 2 ;;
esac
printf '\nchecks passed\n'
