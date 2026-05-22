---
layout: default
title: Wisp — Privacy Policy
permalink: /privacy-policy/
---

# Wisp — Privacy Policy

**Effective date:** 2026-05-22
**App:** Wisp (`dev.vigov5.wisp`)
**Maintainer:** Nguyen Anh Tien
**Contact:** nguyenanhtien2210@gmail.com
**Source code:** <https://github.com/vigov5/wisp>

Wisp is a free, open-source, peer-to-peer file-sharing app built on
[iroh](https://www.iroh.computer/). It is designed so that file
transfers happen **directly between your devices**, end-to-end
encrypted, with no copy of your files ever stored on any server we
or anyone else operates.

This document explains, in plain language, what data Wisp does and
does not handle.

## 1. Data we collect

**None.**

- Wisp does not have user accounts. You do not sign in, register, or
  provide an email or phone number to use it.
- Wisp does not collect, store, sell, share, or transmit any analytics,
  telemetry, usage statistics, device identifiers, advertising IDs, or
  any other personal information.
- Wisp does not use any third-party analytics SDK, advertising SDK,
  crash-reporting service, or tracking pixel.
- Your files do not leave your devices in any form a third party can
  inspect. Transfers happen over an end-to-end encrypted iroh
  connection that only the sender and receiver can decrypt.

## 2. Data your device generates locally

Wisp keeps the following information **only on your device**, in app
storage that no other app and no remote server can read:

- The device name you set in Settings (used as a label other Wisp
  users see when pairing on the same Wi-Fi).
- The download folder you choose for received files.
- A list of devices you have previously transferred with (so the app
  can offer them as quick-reconnect targets). You can clear this list
  at any time from Settings → Saved devices.
- The optional discovery-server URL if you have customised it.

This data never leaves your device. Uninstalling Wisp removes all of
it.

## 3. Network connections Wisp makes

For file transfer to work, Wisp needs to find another device and
establish a connection to it. It does so in two ways:

### 3.1 Local network (LAN) discovery

When the app is open, Wisp broadcasts and listens on your local
Wi-Fi network using mDNS so that two Wisp devices on the same
network can find each other directly. The broadcasts contain only:

- A short, random device name (the one you set in Settings).
- An ephemeral iroh node identifier used for the encrypted handshake.

No files, file metadata, or personal information are broadcast.

### 3.2 Rendezvous server (optional, used only for pairing)

When you pair two devices that are not on the same Wi-Fi (using the
6-character pairing code), Wisp briefly contacts the public
rendezvous server at <https://rendezvous.wisp.mooo.com> to swap
iroh node identifiers between the two devices so they can establish a
direct, encrypted connection.

The rendezvous server:

- Only sees ephemeral pairing codes and the iroh node identifiers
  used for the handshake. It does not see your IP address as a stable
  identifier (it is treated as transient connection metadata).
- Does **not** see your files, file names, file sizes, recipient list,
  or anything else about the transfer.
- Does not log requests beyond what is needed to debug operational
  issues, and any such logs are short-lived and not shared with
  anyone.

If you do not want to use the public rendezvous server you can
either:

- Pair offline using the QR-code scanning feature (same Wi-Fi only,
  no server involved), or
- Run your own rendezvous server (the code is open-source) and point
  Wisp at it in Settings → Advanced → Discovery Server.

### 3.3 iroh relay servers

When two paired devices cannot reach each other directly because of
NAT or firewall restrictions, iroh may route the encrypted connection
through a relay server provided by the iroh project. Relay servers
forward encrypted bytes only — they cannot decrypt your files or see
their contents.

For details, see the
[iroh privacy documentation](https://www.iroh.computer/docs/concepts/privacy).

## 4. Android permissions and why Wisp asks for them

| Permission | Why Wisp needs it |
|---|---|
| `INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE` | To open a network socket and check whether you are online so the app can pick LAN or rendezvous pairing. |
| `CHANGE_WIFI_MULTICAST_STATE` | To send and receive the mDNS multicast packets used for LAN discovery. |
| `NEARBY_WIFI_DEVICES` (Android 13+) | To do mDNS LAN discovery on Android 13+ **without** triggering the location-permission gate. We declare the `neverForLocation` flag — Wisp does not derive your location from this. |
| `ACCESS_FINE_LOCATION`, `ACCESS_COARSE_LOCATION` | Fallback for Android 12 and below, where mDNS discovery requires the location permission. Wisp never reads, stores, or sends your location. |
| `CAMERA` | Only used when you open the QR-scan screen for offline pairing. The camera is never accessed at any other time. No image is recorded or transmitted. |
| `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_DATA_SYNC`, `WAKE_LOCK` | To keep an active iroh transfer running when the app is in the background, so a long file send/receive is not killed by Android. |
| `POST_NOTIFICATIONS` | To show the foreground-service notification required by Android while a transfer is running. |
| `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` | Optional — if you tap the "improve background reliability" prompt, Wisp opens the system dialog so you can exempt the app from battery optimisation. We do not bypass anything without your tap. |

You can revoke any of these at any time in your device Settings →
Apps → Wisp → Permissions; only LAN/QR discovery will be affected.

## 5. Children's privacy

Wisp is not directed at children under 13. We do not knowingly
collect personal information from anyone, and there is no signup, so
no age verification is performed.

## 6. Data sharing

We do not share any data with third parties because we do not collect
any in the first place.

## 7. Open source

Wisp is open-source under the MIT licence. You can read the full
source code at
<https://github.com/vigov5/wisp> and verify the claims made here
against the implementation.

## 8. Changes to this policy

If we change this policy in a way that affects what Wisp does on
your device, we will update the "Effective date" at the top of this
page and note the change in the release notes for the affected
version. Past versions of this policy are tracked in git history.

## 9. Contact

For privacy-related questions, security reports, or to request that
a piece of information be reviewed or removed, write to
**nguyenanhtien2210@gmail.com** or open a confidential issue on the
[GitHub repository](https://github.com/vigov5/wisp/issues).
