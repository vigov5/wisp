#!/usr/bin/env python3
"""Harness for the "share ~91 photos into Wisp" bugs, on a connected device.

Generates a batch of realistic-sized JPEGs and pushes them into the device
gallery so a share of an arbitrary number of photos can be staged repeatably.

Two distinct defects were found with it:

1. The share arrived empty.  Google Photos delivers a small selection with both
   `EXTRA_STREAM` and `ClipData`, but a large one (91 items) with a full
   `ClipData` and no usable `EXTRA_STREAM`.  `MainActivity` read only
   `EXTRA_STREAM`, so the whole batch was silently dropped and the app opened
   on the home screen.  Confirmed by contrast: 3 photos worked, 91 did not, and
   the in-app picker — which reads `ClipData` — handled all 91 fine.

2. `ForegroundServiceDidNotStartInTimeException` killed the process during a
   long send (seen in the device dropbox on v2.1.0):

       Context.startForegroundService() did not then call Service.startForeground()
       ServiceRecord{... dev.vigov5.wisp/.TransferKeepaliveService}

   Every `startForegroundService()` opens a ~5s window in which the framework
   demands a matching `startForeground()`, and progress ticks were routed
   through it about once a second.  `watch` prints the service's `lastStartId`:
   if it climbs while a transfer runs, each tick is arming another deadline.

Repro (share path):
    python tools/repro_share_many_photos.py generate --count 91
    python tools/repro_share_many_photos.py push
    # On the device: Google Photos -> WispRepro album -> long-press one photo
    # -> tap the "Today" circle to select all -> Share -> Wisp.
    # Expect the Send draft with "91 items"; the bug showed the home screen.
    adb logcat -s WispShare        # logs the URI count the intent carried

Usage:
    python tools/repro_share_many_photos.py generate --count 91
    python tools/repro_share_many_photos.py push
    python tools/repro_share_many_photos.py watch
    python tools/repro_share_many_photos.py clean
"""

from __future__ import annotations

import argparse
import random
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

PACKAGE = "dev.vigov5.wisp"
SERVICE = f"{PACKAGE}/.TransferKeepaliveService"
DEVICE_DIR = "/sdcard/Pictures/WispRepro"
LOCAL_DIR = Path(__file__).resolve().parent.parent / "tmp" / "wisp-repro-photos"


def adb(*args: str, check: bool = True) -> str:
    proc = subprocess.run(
        ["adb", *args], capture_output=True, text=True, errors="replace"
    )
    if check and proc.returncode != 0:
        raise SystemExit(f"adb {' '.join(args)} failed:\n{proc.stderr.strip()}")
    return proc.stdout


def generate(count: int, megapixels: float) -> None:
    from PIL import Image

    LOCAL_DIR.mkdir(parents=True, exist_ok=True)
    # 4:3, sized so each JPEG lands in the few-MB range a phone camera produces.
    height = int((megapixels * 1_000_000 / (4 / 3)) ** 0.5)
    width = int(height * 4 / 3)
    rng = random.Random(0x1234)

    for index in range(count):
        # Photographic noise, not flat colour: a flat image compresses to a few
        # KB and would not load the device the way real photos do.
        pixels = bytes(rng.randrange(256) for _ in range(4096 * 3))
        tile = Image.frombytes("RGB", (64, 64), pixels)
        image = tile.resize((width, height), Image.Resampling.BILINEAR)
        path = LOCAL_DIR / f"WISP_REPRO_{index:04d}.jpg"
        image.save(path, "JPEG", quality=92)

    total = sum(p.stat().st_size for p in LOCAL_DIR.glob("*.jpg"))
    print(
        f"generated {count} images in {LOCAL_DIR} "
        f"({total / 1024 / 1024:.1f} MB, {width}x{height})"
    )


def push() -> None:
    if not LOCAL_DIR.exists():
        raise SystemExit("no generated images — run `generate` first")
    adb("shell", "mkdir", "-p", DEVICE_DIR)
    print(f"pushing {LOCAL_DIR} -> {DEVICE_DIR} ...")
    # MSYS would rewrite the leading slash of the device path into a Windows
    # path; adb push is invoked directly (no shell) so the path survives.
    adb("push", str(LOCAL_DIR) + "/.", DEVICE_DIR)
    # Make MediaStore (and therefore Google Photos / the share sheet) see them.
    adb(
        "shell",
        "am",
        "broadcast",
        "-a",
        "android.intent.action.MEDIA_SCANNER_SCAN_FILE",
        "-d",
        f"file://{DEVICE_DIR}",
    )
    adb("shell", "content", "call", "--uri", "content://media", "--method", "scan_volume",
        "--arg", "external_primary", check=False)
    count = adb(
        "shell",
        f"content query --uri content://media/external/images/media "
        f"--projection _id --where \"_data LIKE '%WispRepro%'\" | wc -l",
    ).strip()
    print(f"pushed; MediaStore now lists {count} WispRepro images")


_START_ID = re.compile(r"lastStartId=(\d+)")


def watch(seconds: int) -> None:
    """Print the keepalive service's lastStartId once a second.

    A climbing value is the bug: it counts startForegroundService() calls, each
    of which arms a fresh 5s ForegroundServiceDidNotStartInTime deadline.
    """
    print("watching TransferKeepaliveService (Ctrl-C to stop)")
    print(f"{'t':>5}  {'lastStartId':>11}  state")
    started = time.monotonic()
    first_id: int | None = None
    while time.monotonic() - started < seconds:
        dump = adb("shell", "dumpsys", "activity", "services", SERVICE, check=False)
        match = _START_ID.search(dump)
        elapsed = time.monotonic() - started
        if match is None:
            print(f"{elapsed:5.0f}  {'-':>11}  not running")
        else:
            start_id = int(match.group(1))
            if first_id is None:
                first_id = start_id
            note = "" if start_id == first_id else f"  <-- +{start_id - first_id} restarts"
            print(f"{elapsed:5.0f}  {start_id:>11}  running{note}")
        time.sleep(1)


def clean() -> None:
    adb("shell", "rm", "-rf", DEVICE_DIR)
    adb(
        "shell",
        "content",
        "delete",
        "--uri",
        "content://media/external/images/media",
        "--where",
        "\"_data LIKE '%WispRepro%'\"",
        check=False,
    )
    if LOCAL_DIR.exists():
        shutil.rmtree(LOCAL_DIR)
    print("removed generated images from device and host")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    gen = sub.add_parser("generate", help="create the JPEG batch on the host")
    gen.add_argument("--count", type=int, default=91)
    gen.add_argument("--megapixels", type=float, default=8.0)

    sub.add_parser("push", help="copy the batch into the device gallery")

    w = sub.add_parser("watch", help="tail the keepalive service's start count")
    w.add_argument("--seconds", type=int, default=180)

    sub.add_parser("clean", help="delete the batch from device and host")

    args = parser.parse_args()
    if args.command == "generate":
        generate(args.count, args.megapixels)
    elif args.command == "push":
        push()
    elif args.command == "watch":
        watch(args.seconds)
    elif args.command == "clean":
        clean()
    return 0


if __name__ == "__main__":
    sys.exit(main())
