#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/telemetry.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/telemetry" ;;
    *) rel="x86_64-apple-darwin/telemetry" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/telemetry" ;;
    *) rel="x86_64-unknown-linux-musl/telemetry" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/telemetry.exe" "$here/target/release/telemetry" "$here/target/debug/telemetry.exe" "$here/target/debug/telemetry"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "telemetry binary not found in $here" >&2
exit 1
