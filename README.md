> [!NOTE]
> **Wisp** is a friendly fork of [Drift](https://github.com/vsamarth/drift) by
> Samarth Verma. Heavy ❤️ to the upstream project — Wisp diverges in UX
> direction (Android-first polish, QR code pairing, self-diagnose tooling) and runs its own
> rendezvous infrastructure so we don't lean on Drift's resources.

> [!WARNING]
> Wisp is still rough around the edges. If something breaks, feels confusing, or does not work on your device, please open an issue:
> https://github.com/vigov5/wisp/issues/new

<p align="center">
  <img src="flutter/assets/wisp_rounded_logo.png" width="96" alt="Wisp Logo">
</p>

<h1 align="center">Wisp</h1>

<p align="center">
  <strong>AirDrop-like file sharing for any device, anywhere.</strong>
</p>

<p align="center">
  <img src="flutter/assets/demo.gif" width="500" alt="Wisp Demo">
</p>

Wisp is a free and open-source app for sending files directly between devices, built using [iroh](https://www.iroh.computer/).

It is designed to feel as simple as AirDrop, but without being limited to Apple devices or nearby-only transfers. Pick files, connect to another device, and send.

## Features

- **Send files between devices, near or far**
  Discover nearby devices on your local network, or connect using a 6-character pairing code.

- **Offline pairing via QR**
  No internet? Scan a QR code shown on the receiver to pair over the same Wi-Fi without going through the rendezvous server.

- **Saved devices**
  After a successful transfer, the other device shows up in a "Recent" list so you can pick it again without re-scanning or re-typing a code. (Auto-approve / trusted-device flow is on the roadmap.)

- **Resumable transfers**
  Connection died mid-transfer? Send the same files again and Wisp will resume from where the transfer stopped instead of starting over.

- **Cross-platform**
  Wisp currently provides builds for macOS, Windows, Linux, and Android. iOS support is planned.

- **End-to-end encrypted connections**
  Files are sent over an end-to-end encrypted peer-to-peer connection. Files are never stored in the cloud, and only the sender and receiver can read them.

- **Self-diagnose**
  A built-in Connection Test surfaces network, rendezvous, LAN, and permission issues with actionable hints when things go wrong.

- **Free and open source**
  Wisp is MIT-licensed and open to contributions. No ads, accounts, or limits on what you send.

## Installation

| Platform | Download |
| --- | --- |
| macOS | [Latest release →](https://github.com/vigov5/wisp/releases/latest) |
| Windows | [Latest release →](https://github.com/vigov5/wisp/releases/latest) |
| Linux | [Latest release →](https://github.com/vigov5/wisp/releases/latest) |
| Android | <a href="https://play.google.com/store/apps/details?id=dev.vigov5.wisp"><img src="https://upload.wikimedia.org/wikipedia/commons/7/78/Google_Play_Store_badge_EN.svg" alt="Get it on Google Play" height="40"></a> · [APK (sideload) →](https://github.com/vigov5/wisp/releases/latest) |
| iOS | Coming soon |

> [!TIP]
> **macOS:** Wisp is currently unsigned. If Gatekeeper blocks the app, you can remove the quarantine flag:
>
> ```sh
> xattr -rd com.apple.quarantine /Applications/Wisp.app
> ```

### Build from source

The Flutter app lives in [`flutter/`](flutter/).

See [`flutter/README.md`](flutter/README.md) for build instructions.

## Getting started

1. Choose or drop the files you want to send.
2. Pick a recipient — one of:
   - a nearby device discovered on your LAN,
   - the 6-character pairing code shown on the receiving device, or
   - the QR code shown on the receiving device (scan it from the sender to pair offline, no internet needed).
3. The receiver reviews the files and accepts the transfer.
4. Wisp sends the files directly to the other device.

## Contributing

Wisp is usable, but still early. Contributions, testing, bug reports, and UX feedback are welcome.

Some of the things planned next:

- [x] Resumable transfers for interrupted sessions
- [x] Remember recent devices for quick re-send
- [x] Self-diagnose connection test
- [x] Offline QR pairing
- [ ] Trusted devices with auto-approve (skip the accept prompt on known peers)
- [ ] Dark / light theme
- [ ] Keep Wisp listening in the background
- [ ] Set up app distribution through app stores and package managers
- [ ] Add iOS support

## License

Wisp is licensed under the MIT License. See [`LICENSE`](LICENSE). The original
copyright (Drift, Samarth Verma) is preserved alongside the Wisp fork's
copyright per MIT's notice requirement.
