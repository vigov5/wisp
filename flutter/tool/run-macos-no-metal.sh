#!/usr/bin/env bash
set -euo pipefail

# Run/build the Wisp macOS app with Impeller DISABLED.
#
# Why: Flutter (>= 3.29) renders macOS with Impeller, which needs a working
# Metal stack. On Macs without usable Metal (VMs such as UTM/QEMU/VMware, or
# very old GPUs) the default build loads to a blank/white window or crashes.
# Disabling Impeller falls back to the legacy Skia renderer, which works in
# those environments.
#
# CI is NOT affected: this script only edits macos/Runner/Info.plist for the
# duration of the run and restores the original file on exit (even on Ctrl-C
# or error). The committed Info.plist keeps Impeller on, so release/CI builds
# are unchanged.
#
# Usage (run on the macOS test machine, from anywhere in the repo):
#   flutter/tool/run-macos-no-metal.sh            # debug: flutter run -d macos
#   flutter/tool/run-macos-no-metal.sh build      # release: flutter build macos
#   flutter/tool/run-macos-no-metal.sh profile    # profile build

MODE="${1:-run}"   # run | build | profile

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FLUTTER_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PLIST="$FLUTTER_DIR/macos/Runner/Info.plist"

if [[ ! -f "$PLIST" ]]; then
  echo "✗ Info.plist not found at $PLIST" >&2
  exit 1
fi

BACKUP="$(mktemp)"
cp "$PLIST" "$BACKUP"
restore() {
  cp "$BACKUP" "$PLIST"
  rm -f "$BACKUP"
  echo "↩︎  Restored original Info.plist (Impeller re-enabled)"
}
trap restore EXIT

# Disable Impeller: add the key if missing, otherwise overwrite it.
/usr/libexec/PlistBuddy -c "Add :FLTEnableImpeller bool false" "$PLIST" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c "Set :FLTEnableImpeller false" "$PLIST"
echo "🚫  Impeller disabled (FLTEnableImpeller=false) — this run only"

cd "$FLUTTER_DIR"
case "$MODE" in
  run)     flutter run -d macos ;;
  build)   flutter build macos --release ;;
  profile) flutter build macos --profile ;;
  *) echo "usage: $0 [run|build|profile]" >&2; exit 2 ;;
esac
