# wisp-trimui — Wisp receiver for the TrimUI Brick Pro (TG4040)

A **receive-only** Wisp client that runs on the handheld itself: you pick the
device on your phone or laptop, and the files land on the SD card.

Sending is deliberately not implemented — see [Scope](#scope).

## Scope

| | Feature |
| --- | --- |
| R1 | Receive with the 6-character pairing code |
| R2 | On-screen QR for offline pairing (no internet, same Wi-Fi) |
| R3 | mDNS advertising, so the sender's "nearby" list finds the handheld |
| R4 | Receive text and links, shown on screen |
| R5 | Choose the save folder; rename-or-reject on a name clash |
| R6 | Auto-accept from trusted senders |
| X1 | Resumable transfers (inherited from `wisp-core`, nothing extra to do) |

The interface is English by default, with Vietnamese one row away in
*Settings → Language*. Both are compiled in; see `i18n.rs`.

Not included: sending, identity export/import, the connection test, and a
background/boot service.

## Why not SDL2

TrimUI publishes SDKs for the Smart Pro (TG5040) and Smart Pro S (TG5050) but
**not for the Brick Pro (TG4040)**, so linking against the device's SDL2 would
mean guessing at its version and patches. The stock firmware — Allwinner Tina
Linux — also exposes a plain framebuffer with no DRM and no Wayland:

* `/dev/fb0` — 1024x768, 32bpp, 4096-byte stride
* `/dev/input/event*` — the pad enumerates as an Xbox 360 controller (045e:028e)

Drawing straight to that framebuffer and reading evdev directly means the whole
app is pure Rust with no C dependencies, which in turn means it links
**statically against musl**: one binary that does not care whether the rootfs
is musl or glibc, and so runs on stock, MinUI/NextUI and Knulli alike.

The cost is that text, QR codes, rounded rectangles and scrolling are all
implemented here (`draw.rs`, `font.rs`, `qr.rs`, `ui.rs`). They are covered by
unit tests, and `examples/preview.rs` renders every screen to a PNG so layout
can be reviewed without the hardware.

## Build

The target is `aarch64-unknown-linux-musl`, statically linked. Not
`...-gnu`: the device is on glibc 2.33 and any modern build host links against
something newer, so a dynamic binary would refuse to start on the handheld.

`ring` (via `iroh`) compiles C, so the build needs an aarch64 C compiler.
Either way works:

**On any aarch64 Linux host** — no cross toolchain, just musl:

```sh
sudo apt install musl-tools
rustup target add aarch64-unknown-linux-musl
export CC_aarch64_unknown_linux_musl=musl-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cargo build -p wisp-trimui --release --target aarch64-unknown-linux-musl
```

Then package it from a checkout anywhere:

```sh
make trimui-pak TRIMUI_BIN=/path/to/wisp-trimui
```

**Cross-compiling from x86-64** with [`cross`](https://github.com/cross-rs/cross)
and Docker, whose image carries the toolchain:

```sh
cargo install cross --git https://github.com/cross-rs/cross
make trimui-pak
```

Either produces `target/trimui/Wisp/`:

```
Wisp/
├── wisp-trimui     # static aarch64 binary
├── launch.sh       # stock-firmware entry point
├── config.json     # launcher manifest
└── icon.png
```

To review the UI without a device:

```sh
make trimui-preview   # -> target/preview/*.png
```

## Install

Copy the folder to `/mnt/SDCARD/Apps/Wisp/` on the card, then restart the
launcher — MainUI only scans `Apps/` at startup, and it ignores `SIGTERM`:

```sh
make trimui-install TRIMUI_HOST=root@192.168.1.50
```

Or by hand: drop the folder on the card, reboot, and Wisp appears in the Apps
list. Enable SSH first in the stock settings (it is stored in
`/mnt/UDISK/system.json`).

## Using it

The app opens on the waiting screen: pairing code on the left, QR on the right,
and underneath it the save folder, the device name, this handheld's **identity**
(the short form of its public key, which is what a sender remembers it by), and
the LAN address.

| Button | Waiting | Incoming request | Transferring | Result | Menus |
| --- | --- | --- | --- | --- | --- |
| A | New code | Accept | — | Done | Select |
| B | Exit | Decline | Cancel | Done | Back |
| X | — | Trust + accept | — | Trust / untrust sender | Remove (trusted list) |
| Y | Settings | — | — | — | — |
| D-pad | — | Scroll files | — | Scroll | Move |

Received files go to `/mnt/SDCARD/Wisp` unless changed in Settings. The folder
picker lists the last 30 folders you chose before the built-in suggestions, so
a destination reached once through the browser is one press away afterwards.

The last Settings row, *About*, shows the version and this handheld's **full**
identity key — untruncated on purpose, so it can be compared against what the
sending device displays.

## Settings file

`/mnt/SDCARD/.wisp/settings.json`, written by the app and safe to edit over
SSH:

```json
{
  "device_name": "Brick Pro",
  "lang": "en",
  "save_root": "/mnt/SDCARD/Roms",
  "conflict": "rename",
  "server": null,
  "trusted": [{ "endpoint_id": "…", "name": "Pixel 7" }],
  "recent_save_roots": ["/mnt/SDCARD/Roms/FC", "/mnt/SDCARD/Wisp"],
  "secret_key": "…",
  "button_overrides": {}
}
```

`secret_key` is the device's Wisp identity. It is generated on first run and
must be kept: senders remember this handheld by its public key, so deleting it
makes every saved entry on the sending side go stale.

The log is next to it, at `/mnt/SDCARD/.wisp/wisp-trimui.log`.

## Troubleshooting

**The buttons do the wrong thing.** The default table was measured on a TG4040
(see [Device facts](#device-facts)), but a different firmware may renumber
them. Open *Settings → Kiểm tra nút*, press each button, note the raw code, and
add it to `button_overrides` (evdev code as a string, button name as the
value):

```json
"button_overrides": { "305": "A", "304": "B" }
```

Valid names: `Up Down Left Right A B X Y L1 R1 L2 R2 Start Select Menu Power`.

**"… is not on a mounted card", or only Wisp appears in the Apps menu.**
`/mnt/SDCARD` is a mount point, and the stock firmware does not make a failed
mount visible: when the card is missing or busy, that path is still a
perfectly writable directory on the internal overlay
(`/overlay/upper/mnt/SDCARD`). Anything written there goes to internal
storage, and the card shadows it again the moment it mounts — so a file can be
received, verified and reported as saved while being nowhere the user can find
it. That is also why a machine whose card failed to mount can show *only* Wisp
in Apps: the app was once installed through that window, so a copy of it is
sitting on internal storage while the games exist only on the card.

Both `launch.sh` and the app check `/proc/mounts` before creating anything
under `/mnt/SDCARD` and refuse to run otherwise. To find files stranded by an
earlier occurrence:

```sh
ls -laR /overlay/upper/mnt/SDCARD
```

Anything there is on internal storage. Move it onto the real card, then
delete the shadowed copy.

**Nothing on screen / it exits immediately.** Check
`/mnt/SDCARD/.wisp/launch.log`. Opening `/dev/fb0` needs root and needs MainUI
to have exited; running the binary over SSH while MainUI is up will fail or
fight it for the screen. `WISP_TRIMUI_FB=/dev/fb1` overrides the node.

**No pairing code, "Không có mạng".** The short code needs the rendezvous
server, so it needs internet. QR pairing and nearby discovery both work on a
LAN with no internet — use those.

**Nothing to verify against.** `tools/trimui-probe.sh` dumps the framebuffer
geometry, input devices, libc, storage layout and the stock `config.json`
schema in one go:

```sh
make trimui-probe TRIMUI_HOST=root@192.168.1.50   # -> probe.txt
```

## Device facts

Measured on a TG4040 running stock firmware, not taken from documentation.
`tools/trimui-probe.sh` re-checks all of it.

| | |
| --- | --- |
| Kernel / libc | Linux 4.9.191 aarch64, glibc 2.33 |
| Memory / cores | 998 MB, 4 |
| Framebuffer | `/dev/fb0`, 1024x768 visible, 1024x16384 virtual, 4096 stride, 32bpp |
| Pixel layout | `rgba 8/16,8/8,8/0,8/24` — `0xAARRGGBB`, alpha must be opaque |
| Pad | `event3`, "TRIMUI Player1" (045e:028e); D-pad on `ABS_HAT0X/Y` |
| Face buttons | A=`0x131`, B=`0x130`, X=`0x134`, Y=`0x133` — see below |
| Stick drift at rest | ~4 500 on X, ~-2 900 on Y, emitted continuously |
| Storage | `/mnt/SDCARD` (SD), `/mnt/UDISK` (internal) |
| SD filesystem | **vfat (FAT32)**, mounted `sync`, everything mode 0777 |

The FAT32 card is worth knowing about: **a single received file cannot exceed
4 GB**, whatever the transfer says, and the card carries no POSIX permissions —
`chmod` is a no-op there, which is also why `launch.sh` stays executable
without anyone setting a bit.

The face buttons are the one genuine trap. The pad enumerates as an Xbox 360
controller, so the kernel calls them `BTN_A`/`BTN_B`/`BTN_X`/`BTN_Y` — but the
codes follow Xbox *positions* while the shell is silkscreened Nintendo-style.
The button printed "A" therefore sends `BTN_B`. Mapping by kernel name would
swap confirm and cancel; `input.rs` maps by silkscreen instead.

## Still unverified

Everything above was exercised on hardware, including `fb.rs` opening the real
framebuffer and its output being read back and inspected. What has *not* run on
the device yet:

* the app itself — `ring` needs an aarch64 C compiler, so the full binary has
  not been cross-compiled (the framebuffer and input modules were, and ran)
* whether MainUI lists the app from `pak/config.json` (the schema was copied
  from the stock apps on the device, but not yet round-tripped)
* a real transfer: throughput, and whether 998 MB of RAM is comfortable
