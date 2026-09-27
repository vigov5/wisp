#!/bin/sh
# TrimUI Brick Pro (TG4040) stock-firmware launcher.
#
# MainUI writes /tmp/cmd_to_run.sh and exits before this runs, so the
# framebuffer is already free by the time wisp-trimui opens it. When this
# script returns, runtrimui.sh loops around and brings MainUI back.

# `echo $0 $*` and the progdir/cd/LD_LIBRARY_PATH preamble are what the stock
# apps do; kept so this behaves like the rest of Apps/ even though a static
# binary does not need the library path.
echo "$0" "$@"
progdir=$(dirname "$0")
cd "$progdir" || exit 1
export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:$progdir"

STATE_DIR="/mnt/SDCARD/.wisp"

# This check has to come before anything creates a directory under
# /mnt/SDCARD. The card is a mount point, and when it fails to mount
# /mnt/SDCARD stays a perfectly ordinary directory on the internal overlay:
# writing there puts files on internal storage, which the card then hides the
# moment it mounts again. Even `mkdir -p "$STATE_DIR"` would leave a stray
# directory behind, so the guard runs first and the log line goes to /tmp.
if ! grep -q " /mnt/SDCARD " /proc/mounts; then
    echo "SD card is not mounted; refusing to start" >> /tmp/wisp-launch.log
    exit 1
fi

mkdir -p "$STATE_DIR"

# The app draws straight to /dev/fb0 and reads /dev/input/event*, both of
# which need root. Stock firmware already runs apps as root; this only makes
# the failure legible if that ever stops being true.
if [ "$(id -u)" != "0" ]; then
    echo "wisp-trimui needs root for /dev/fb0 and /dev/input" >> "$STATE_DIR/launch.log"
fi

chmod +x ./wisp-trimui 2>/dev/null

# stdout/stderr only carry startup failures — the app's own log goes to
# $STATE_DIR/wisp-trimui.log once tracing is up.
./wisp-trimui >> "$STATE_DIR/launch.log" 2>&1
