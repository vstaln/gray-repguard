#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-repguard.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-repguard" ;;
    *) rel="x86_64-apple-darwin/gray-repguard" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-repguard" ;;
    *) rel="x86_64-unknown-linux-musl/gray-repguard" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-repguard.exe" "$here/target/release/gray-repguard" "$here/target/debug/gray-repguard.exe" "$here/target/debug/gray-repguard"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-repguard binary not found in $here" >&2
exit 1
