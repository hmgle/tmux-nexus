#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/tnx-launcher-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

repo="$TMP/repo"
mkdir -p "$repo/src" "$TMP/bin"
cp "$ROOT/tnx" "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" "$repo/"

cat > "$TMP/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '<%s>\n' "$@"
EOF
chmod +x "$TMP/bin/cargo"

output="$(
    env -u TNX_BIN -u TNX_AUTO_DOWNLOAD \
        PATH="$TMP/bin:/usr/bin:/bin" \
        bash "$repo/tnx" sample-argument
)"
expected="$(printf '<run>\n<--release>\n<--quiet>\n<--manifest-path>\n<%s>\n<-->\n<sample-argument>' "$repo/Cargo.toml")"

if [ "$output" != "$expected" ]; then
    echo "launcher test: Cargo fallback did not use the release profile" >&2
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$output" >&2
    exit 1
fi

# A current backend is used, but stale or mismatched backends still rebuild.
mkdir -p "$repo/target/release"
version="$(awk -F'"' '/^version[[:space:]]*=/{print $2; exit}' "$repo/Cargo.toml")"
cat > "$repo/target/release/tnx" <<EOF
#!/usr/bin/env bash
if [ "\$1" = --version ]; then
    printf 'tnx %s\\n' '$version'
else
    printf 'backend:<%s>\\n' "\$1"
fi
EOF
chmod +x "$repo/target/release/tnx"
run_launcher() {
    env -u TNX_BIN PATH="$TMP/bin:/usr/bin:/bin" bash "$repo/tnx" sample-argument
}
[ "$(run_launcher)" = 'backend:<sample-argument>' ]
touch -t 203001010000 "$repo/src/new.rs"
[ "$(run_launcher)" = "$expected" ]
rm "$repo/src/new.rs"
sed "s/$version/999.0.0/" "$repo/target/release/tnx" > "$TMP/wrong-version"
cat "$TMP/wrong-version" > "$repo/target/release/tnx"
[ "$(run_launcher)" = "$expected" ]

# An explicit override works even without a manifest, preserving arguments.
rm "$repo/Cargo.toml"
output="$(TNX_BIN="$TMP/bin/cargo" bash "$repo/tnx" 'argument with spaces')"
[ "$output" = '<argument with spaces>' ]
if TNX_BIN="$TMP/missing" bash "$repo/tnx" >/dev/null 2>&1; then
    echo "launcher test: accepted a missing override" >&2
    exit 1
fi

echo "launcher tests passed"
