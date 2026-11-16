#!/usr/bin/env sh
# Keeps the cargo build directory in check (`just prune`; local Claude Code
# hooks also run it after test runs and when a session ends). Each integration test is its own executable of a
# few hundred MB, so builds grow fast.
#
# - under PRUNE_SOFT_GB (15): nothing is removed;
# - above it: test executables and incremental caches are removed; compiled
#   dependencies stay, so the next run only rebuilds and links the tests;
# - still above PRUNE_HARD_GB (30): `cargo clean`.
#
# Looks at $CARGO_TARGET_DIR (when set) and at the repository's own target/.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
soft=${PRUNE_SOFT_GB:-15}
hard=${PRUNE_HARD_GB:-30}

size_gb() { du -sk "$1" 2>/dev/null | awk '{ printf "%d", $1 / 1048576 }'; }

prune() {
    dir=$1
    [ -d "$dir" ] || return 0
    before=$(size_gb "$dir")
    if [ "$before" -lt "$soft" ]; then
        echo "prune: $dir is ${before} GB (under ${soft} GB): kept"
        return 0
    fi
    for profile in "$dir"/debug "$dir"/*/debug; do
        [ -d "$profile/deps" ] || continue
        # Executables only (tests, binaries' copies): libraries and proc macros stay.
        find "$profile/deps" -maxdepth 1 -type f -perm -u+x \
            ! -name '*.rlib' ! -name '*.rmeta' ! -name '*.so' ! -name '*.dylib' ! -name '*.dll' -delete
        rm -rf "$profile/incremental"
    done
    after=$(size_gb "$dir")
    if [ "$after" -ge "$hard" ]; then
        cargo clean --manifest-path "$root/Cargo.toml" --target-dir "$dir" >/dev/null 2>&1 || rm -rf "$dir"
        echo "prune: $dir was ${after} GB after removing test executables (over ${hard} GB): cleaned"
    else
        echo "prune: $dir ${before} GB -> ${after} GB (test executables and incremental caches removed)"
    fi
}

[ -n "${CARGO_TARGET_DIR:-}" ] && prune "$CARGO_TARGET_DIR"
[ "${CARGO_TARGET_DIR:-}" = "$root/target" ] || prune "$root/target"
