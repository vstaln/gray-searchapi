#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-searchapi.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-searchapi" ;;
    *) rel="x86_64-apple-darwin/gray-searchapi" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-searchapi" ;;
    *) rel="x86_64-unknown-linux-musl/gray-searchapi" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-searchapi.exe" "$here/target/release/gray-searchapi" "$here/target/debug/gray-searchapi.exe" "$here/target/debug/gray-searchapi"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-searchapi binary not found in $here" >&2
exit 1
