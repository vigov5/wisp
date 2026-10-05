# wisp-trimui — Wisp receiver for the TrimUI Brick Pro (TG4040)

A Wisp client that runs on the handheld itself: receive files onto the SD card
from a phone or laptop, and send files back off it.

Sending is by LAN discovery, a previously used device, or a typed pairing
code. There is no QR pairing in either direction — the handheld has no camera
to scan one with, and the receive screen shows a QR for the *other* device to
scan.

## Scope

| | Feature |
| --- | --- |
| S1 | Send files and folders, picked with an on-device browser |
| S2 | Send to a device found on the LAN, or one sent to before |
| S3 | Send to a typed pairing code (on-screen keyboard) |
| R1 | Receive with the 6-character pairing code |
| R2 | On-screen QR for offline pairing (no internet, same Wi-Fi) |
| R3 | mDNS advertising, so the sender's "nearby" list finds the handheld |
| R4 | Receive text and links, shown on screen |
| R5 | Choose the save folder; rename-or-reject on a name clash |
| R6 | Auto-accept from trusted senders |
| X1 | Resumable transfers (inherited from `wisp-core`, nothing extra to do) |

The interface is English by default, with Vietnamese one row away in
*Settings → Language*. Both are compiled in; see `i18n.rs`.

Not included: identity export/import, the connection test, and a
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
| X | **Send** | Trust + accept | — | Trust / untrust sender | Remove (trusted list) |
| Y | Settings | — | — | — | — |
| D-pad | — | Scroll files | — | Scroll | Move |

Sending: **X** on the waiting screen opens a file picker — **A** enters a
folder or ticks a file, **Y** queues or un-queues a whole folder, **X**
continues. Anything queued, file or folder, is marked in the list.

The destination screen starts scanning the LAN straight away. Its first row is
the scan control and stays put whether scanning or not, so it never shifts
under the cursor; **A** on it (or **Y** anywhere) stops and restarts the
search. Below it are the devices found, then devices sent to before, then
manual code entry — on the keyboard **A** types, **X** deletes and **Start**
sends.

**The handheld does not sleep while Wisp is open.** Suspending drops the
Wi-Fi link, so a sleeping handheld is an unreachable one — it would sit there
showing a pairing code nobody can connect to, and a transfer in flight would
die. The launcher holds `/tmp/stay_awake`, the same flag the stock
musicplayer, moonlight and usb_storage apps use, and releases it on exit.
Close the app to get the battery back.

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

## Verified on hardware

All of it now runs on a TG4040 on stock firmware, launched from the Apps menu
like any other app. Numbers below were measured on the device, not estimated.

| | |
| --- | --- |
| Receive | 351 MB from a laptop in 4 min 30 s — **1.30 MB/s** |
| Receive, over the handheld's own hotspot | ~0.95 MB/s |
| Offer reaching the screen | **6-7 ms** after the sender's offer lands |
| Re-receiving a file already on the card | 440 MB adopted in **7 s**, nothing fetched |
| Send | handheld to a desktop, over LAN |
| Memory during a 1.4 GB receive | 785 MB of 998 MB still available |

The launcher manifest round-trips: MainUI lists the app from `pak/config.json`.

Speed is the radio, not the code. The XR819 is 2.4 GHz only, and a healthy
link here reads -52 dBm at 54-58 MBit/s — while `dmesg` fills with
`[TXRX_WRN] drop=…` under a sustained stream. Writes land on a `sync`-mounted
FAT32 card, so `irq/364-sunxi-m` takes a third of a core during a transfer.

## Still unverified

* the LAN TCP blob path failing *mid-collection*, which should fall back to
  QUIC rather than end the transfer — the fallback has not yet been provoked on
  a device
* QR pairing scanned by a real phone camera, as opposed to the payload being
  decoded from a screenshot

## Getting files onto the device

Worth writing down, because three obvious routes do not work:

* **SFTP** — the device refuses every open-for-write with `ENOENT`, including
  to `/tmp`. `scp` inherits this on OpenSSH 9+, which uses the SFTP subsystem;
  `scp -O` forces the older protocol and avoids it.
* **An SSH exec channel** (`cat > file`) — fine for small files, but a
  sustained write kills the channel and the session with it.
* **The device fetching over HTTP** — works, but only if nothing on the serving
  machine's side blocks inbound connections.

What does work is the device listening and the other end connecting out:

```sh
ssh root@<device> "setsid nohup sh -c 'nc -l -p 9099 >> /tmp/x' >/dev/null 2>&1 &"
# then connect to <device>:9099 and stream
```

12 MB goes across in one round. The difference is that retransmission stays in
TCP instead of riding under an SSH channel that dies with the first stall.
