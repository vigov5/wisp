#!/usr/bin/env bash
# Build the wasm receiver and generate JS bindings into web/pkg/.
#
# Browser iroh pulls `ring`, whose C compiles for wasm only with clang. Set
# CC_wasm32_unknown_unknown / AR_wasm32_unknown_unknown to a clang/llvm-ar, or let
# this script fall back to the Android NDK's bundled clang if $ANDROID_HOME is set.
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

if [[ -z "${CC_wasm32_unknown_unknown:-}" ]]; then
  # Try to locate an NDK clang under $ANDROID_HOME (or the default SDK location).
  sdk="${ANDROID_HOME:-$HOME/AppData/Local/Android/Sdk}"
  clang="$(ls -d "$sdk"/ndk/*/toolchains/llvm/prebuilt/*/bin/clang.exe 2>/dev/null | sort -V | tail -1 || true)"
  if [[ -n "$clang" ]]; then
    export CC_wasm32_unknown_unknown="$clang"
    export AR_wasm32_unknown_unknown="${clang%clang.exe}llvm-ar.exe"
    echo "Using NDK clang: $clang"
  else
    echo "warning: no clang found; set CC_wasm32_unknown_unknown if the build fails on 'ring'." >&2
  fi
fi

PROFILE="${1:-debug}"   # pass 'release' for an optimized, much smaller wasm
if [[ "$PROFILE" == "release" ]]; then
  cargo build --profile wasm-release --target wasm32-unknown-unknown -p wisp-web-receiver
  WASM="target/wasm32-unknown-unknown/wasm-release/wisp_web_receiver.wasm"
else
  cargo build --target wasm32-unknown-unknown -p wisp-web-receiver
  WASM="target/wasm32-unknown-unknown/debug/wisp_web_receiver.wasm"
fi

wasm-bindgen "$WASM" --out-dir web/pkg --target web
# Optional post-pass if binaryen's wasm-opt is on PATH. `-O2` keeps the speed
# optimizations (the receiver is crypto/hashing-bound over the relay); use `-Oz`
# only if you need the smallest possible download and can spare throughput.
if command -v wasm-opt >/dev/null 2>&1 && [[ "$PROFILE" == "release" ]]; then
  wasm-opt -O2 web/pkg/wisp_web_receiver_bg.wasm -o web/pkg/wisp_web_receiver_bg.wasm
fi
echo "Bindings written to web/pkg/ ($(du -h web/pkg/wisp_web_receiver_bg.wasm | cut -f1) wasm)"
