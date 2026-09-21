#!/usr/bin/env bash
# Cargo `runner` wrapper for local macOS dev builds (`cargo run` / `cargo test`,
# including the process `tauri dev` launches).
#
# By default every debug build gets a fresh ad-hoc signature from the linker,
# so macOS treats each rebuild as a different app: Accessibility and
# Microphone grants in System Settings silently stop applying and have to be
# re-granted after every `cargo build`. Re-signing with a stable local
# certificate ("Handy Local Dev", a self-signed cert created once via Keychain
# Access) keeps the code identity constant across rebuilds, so TCC grants
# persist.
#
# Only touches machines that actually have that certificate installed — CI
# and other contributors' machines don't, so this is a silent no-op there and
# the binary just runs with its default signature.
set -euo pipefail

bin="$1"
shift

identity="Handy Local Dev"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if security find-identity -v -p codesigning 2>/dev/null | grep -q "\"$identity\""; then
  codesign --force -s "$identity" \
    --entitlements "$repo_root/src-tauri/Entitlements.plist" \
    "$bin"
fi

exec "$bin" "$@"
