#!/usr/bin/env bash
#
# sideload-ipa.sh — install an unsigned (CI, --no-codesign) IPA on a JAILBROKEN
# iPhone so the app gets a real sandbox container (Metal/GPU works — a plain
# /var/jb/Applications drop-in does NOT and Flutter aborts with
# "Metal may only be unavailable on simulators").
#
# Modes:
#   USB : Mac + cable, installs through installd with ideviceinstaller (needs ldid).
#   SSH : over Wi-Fi. Prefers TrollStore Lite's trollstorehelper (self-signs +
#         proper container, fully automatic). If TrollStore isn't installed it
#         falls back to fake-signing with ldid and leaves signed.ipa for a
#         one-tap Filza install.
#
# Both fake-signing paths also sign every nested PlugIns/*.appex (Wisp ships a
# Share Extension) and grant the app group the extension uses to hand shares to
# the app. An unsigned .appex installs fine and then gets killed the instant
# it's invoked, which shows up as "tapping Wisp in the share sheet crashes".
#
# One-time prerequisites:
#   iPhone : AppSync Unified (github.com/akemin-dayo/AppSync releases, rootless =
#            iphoneos-arm64). Optional but recommended: TrollStore Lite
#            (apt package com.opa334.trollstorelite) for hands-off Wi-Fi installs.
#   USB    : brew install ldid libimobiledevice ; device trusted over USB.
#
# Usage:
#   ./sideload-ipa.sh wisp.ipa                       # USB
#   ./sideload-ipa.sh --ssh 192.168.1.79 wisp.ipa    # Wi-Fi (prompts for root pw)
#   ./sideload-ipa.sh --ssh 192.168.1.79 --pass 1 wisp.ipa
#
set -euo pipefail

MODE=usb ; HOST="" ; PASS="" ; IPA=""
while [ $# -gt 0 ]; do
  case "$1" in
    --ssh)  MODE=ssh ; HOST="${2:?--ssh needs an ip/host}" ; shift 2 ;;
    --pass) PASS="${2:?--pass needs a value}" ; shift 2 ;;
    -h|--help) grep '^#' "$0" | cut -c3- ; exit 0 ;;
    *) IPA="$1" ; shift ;;
  esac
done
# on Git Bash (Windows) accept a C:\... or C:/... path by converting to /c/...
if command -v cygpath >/dev/null 2>&1; then
  case "$IPA" in [A-Za-z]:[\\/]*) IPA="$(cygpath -u "$IPA")";; esac
fi
[ -n "$IPA" ] && [ -f "$IPA" ] || { echo "usage: sideload-ipa.sh [--ssh HOST [--pass PW]] <file.ipa>" >&2; exit 1; }

# ---------------------------------------------------------------- USB mode -----
if [ "$MODE" = usb ]; then
  command -v ldid >/dev/null            || { echo "missing ldid -> brew install ldid" >&2; exit 1; }
  command -v ideviceinstaller >/dev/null|| { echo "missing ideviceinstaller -> brew install libimobiledevice" >&2; exit 1; }

  WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
  echo "==> unzip"; unzip -q "$IPA" -d "$WORK"
  APP="$(echo "$WORK"/Payload/*.app)"; [ -d "$APP" ] || { echo "no .app in Payload/" >&2; exit 1; }
  BID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Info.plist")"
  EXE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Info.plist")"
  echo "    bundle=$BID exe=$EXE"

  # The App Group backing the Share Extension's hand-off to the app. Both the
  # app and the .appex must carry it or containerURL(forSecurityApplicationGroupIdentifier:)
  # returns nil and shared items go nowhere. Mirrors Runner.entitlements.
  GROUP="group.${BID}"

  cat > "$WORK/ent.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>${BID}</string>
  <key>com.apple.developer.team-identifier</key><string>0000000000</string>
  <key>get-task-allow</key><true/>
  <key>keychain-access-groups</key><array><string>${BID}</string></array>
  <key>com.apple.security.application-groups</key><array><string>${GROUP}</string></array>
</dict></plist>
EOF

  echo "==> fake-sign frameworks"
  if [ -d "$APP/Frameworks" ]; then
    find "$APP/Frameworks" -type f \( -name '*.dylib' -o \( -path '*.framework/*' ! -name '*.*' \) \) -print0 |
      while IFS= read -r -d '' f; do ldid -S "$f"; done
  fi

  # App extensions are separate signed binaries nested in PlugIns/. Leaving one
  # unsigned doesn't fail the install — the extension just gets killed the
  # instant it's invoked, which looks like "tapping Wisp in the share sheet
  # crashes". Sign nested code BEFORE the outer app.
  for EXT in "$APP"/PlugIns/*.appex; do
    [ -d "$EXT" ] || continue
    EXT_BID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$EXT/Info.plist")"
    EXT_EXE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$EXT/Info.plist")"
    echo "==> fake-sign extension $EXT_BID"
    cat > "$WORK/ext.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>${EXT_BID}</string>
  <key>com.apple.developer.team-identifier</key><string>0000000000</string>
  <key>get-task-allow</key><true/>
  <key>com.apple.security.application-groups</key><array><string>${GROUP}</string></array>
</dict></plist>
EOF
    ldid -S"$WORK/ext.plist" "$EXT/$EXT_EXE"; chmod 0755 "$EXT/$EXT_EXE"
  done

  echo "==> fake-sign main binary"; ldid -S"$WORK/ent.plist" "$APP/$EXE"; chmod 0755 "$APP/$EXE"
  echo "==> repackage"; SIGNED="$WORK/signed.ipa"; ( cd "$WORK" && zip -qry "$SIGNED" Payload )
  echo "==> install via installd"; ideviceinstaller -i "$SIGNED"
  echo "==> done — launch $BID on the device."
  exit 0
fi

# ---------------------------------------------------------------- SSH mode -----
SSHDIR="$(mktemp -d)"; trap 'rm -rf "$SSHDIR"' EXIT
SSHOPTS=(-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$SSHDIR/known_hosts" \
         -o ConnectTimeout=25 -o PreferredAuthentications=password -o PubkeyAuthentication=no)
if [ -n "$PASS" ]; then
  printf '#!/bin/sh\necho %s\n' "$PASS" > "$SSHDIR/askpass.sh"; chmod +x "$SSHDIR/askpass.sh"
  export SSH_ASKPASS="$SSHDIR/askpass.sh" SSH_ASKPASS_REQUIRE=force DISPLAY=:0
fi

echo "==> copy IPA to phone"
scp "${SSHOPTS[@]}" "$IPA" "root@$HOST:/var/mobile/_sideload_in.ipa"

echo "==> install on device (over Wi-Fi)"
ssh "${SSHOPTS[@]}" "root@$HOST" 'sh -s' <<'REMOTE'
set -e
export PATH=/var/jb/usr/bin:/var/jb/usr/sbin:/var/jb/bin:/var/jb/sbin:/var/jb/usr/local/bin:$PATH
IN=/var/mobile/_sideload_in.ipa
PB=/var/jb/usr/libexec/PlistBuddy

# read bundle id (used for TrollStore lookup and the ldid fallback)
W=/var/mobile/_sideload; rm -rf "$W"; mkdir "$W"; cd "$W"
unzip -q "$IN"
APP="$(echo Payload/*.app)"
[ -x "$PB" ] || apt-get install -y plistbuddy >/dev/null 2>&1 || true
BID="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleIdentifier' "$APP/Info.plist" 2>/dev/null || true)"
case "$BID" in ""|"(null)") BID="$(basename "$APP" .app)";; esac
echo "    bundle=$BID"

# prefer TrollStore Lite: it self-signs and installs through installd (real container)
TSH=""
for c in /var/jb/Applications/TrollStoreLite.app/trollstorehelper \
         /var/jb/Applications/TrollStore.app/trollstorehelper; do
  [ -x "$c" ] && TSH="$c" && break
done

if [ -n "$TSH" ]; then
  # Entitle the App Group into the payload BEFORE handing it over, and let
  # TrollStore sign the result.
  #
  # A CI --no-codesign build embeds no entitlements, so TrollStore has nothing
  # to carry over and gives nested plugins none — an .appex without
  # `application-identifier` is killed at exec. Patching it after the install
  # instead is worse than useless: TrollStore writes a _CodeSignature over the
  # whole bundle, so re-signing the extension binary afterwards leaves the
  # bundle's CodeResources hashes stale, the extension fails validation, and
  # LaunchServices refuses to launch it —
  #   runningboardd: Failed to get LSApplicationRecord ... Code=-10814
  # while it still shows up in the share sheet. Entitle first, sign once.
  GROUP="group.${BID}"
  if [ -d "$APP/PlugIns" ]; then
    command -v ldid >/dev/null || { apt-get update >/dev/null 2>&1 || true; apt-get install -y ldid >/dev/null 2>&1 || true; }
  fi
  if [ -d "$APP/PlugIns" ] && command -v ldid >/dev/null; then
    for ext in "$APP"/PlugIns/*.appex; do
      [ -d "$ext" ] || continue
      ebid="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleIdentifier' "$ext/Info.plist" 2>/dev/null || true)"
      case "$ebid" in ""|"(null)") ebid="${BID}.$(basename "$ext" .appex)";; esac
      eexe="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleExecutable' "$ext/Info.plist" 2>/dev/null || true)"
      case "$eexe" in ""|"(null)") eexe="$(basename "$ext" .appex)";; esac
      [ -f "$ext/$eexe" ] || continue
      cat > "$W/ext_ent.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>${ebid}</string>
  <key>com.apple.security.application-groups</key><array><string>${GROUP}</string></array>
</dict></plist>
EOF
      echo "==> entitling extension $ebid (pre-install)"
      ldid -S"$W/ext_ent.plist" "$ext/$eexe" && chmod 0755 "$ext/$eexe"
    done
    # The app needs the same group or it can't read what the extension drops.
    cat > "$W/app_ent.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>${BID}</string>
  <key>com.apple.security.application-groups</key><array><string>${GROUP}</string></array>
</dict></plist>
EOF
    AEXE="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleExecutable' "$APP/Info.plist" 2>/dev/null || true)"
    case "$AEXE" in ""|"(null)") AEXE="$(basename "$APP" .app)";; esac
    if [ -f "$APP/$AEXE" ]; then
      echo "==> entitling app $BID (pre-install)"
      ldid -S"$W/app_ent.plist" "$APP/$AEXE" && chmod 0755 "$APP/$AEXE"
    fi
    # Repackage so TrollStore installs the entitled payload, not the original.
    rm -f /var/mobile/_sideload_entitled.ipa
    ( cd "$W" && zip -qry /var/mobile/_sideload_entitled.ipa Payload ) && IN=/var/mobile/_sideload_entitled.ipa
  fi

  echo "==> installing via TrollStore"
  # this build's `install` takes only `install <path>`; to replace an existing
  # app, remove it by container path first (works for TrollStore & non-TrollStore).
  OLD="$(uicache -l 2>/dev/null | grep "^${BID} " | sed 's/.*: //' | head -1)"
  [ -n "$OLD" ] && { echo "    removing existing: $OLD"; "$TSH" uninstall-path "$OLD" >/dev/null 2>&1 || true; }
  OUT="$("$TSH" install "$IN" 2>&1)"
  echo "$OUT" | grep -iE 'created app container|new app path|already installed|error|returning' | tail -4
  rm -f /var/mobile/_sideload_entitled.ipa
  echo "INSTALLED_VIA=trollstore"
else
  echo "==> no TrollStore — fake-signing with ldid (Filza will do the install)"
  command -v ldid >/dev/null || { apt-get update >/dev/null 2>&1 || true; apt-get install -y ldid >/dev/null; }
  EXE="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleExecutable' "$APP/Info.plist" 2>/dev/null || true)"
  case "$EXE" in ""|"(null)") EXE="$(basename "$APP" .app)";; esac
  [ -f "$APP/$EXE" ] || EXE="$(basename "$APP" .app)"
  # App Group backing the Share Extension hand-off; must match Runner.entitlements.
  GROUP="group.${BID}"
  cat > ent.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>${BID}</string>
  <key>com.apple.developer.team-identifier</key><string>0000000000</string>
  <key>get-task-allow</key><true/>
  <key>keychain-access-groups</key><array><string>${BID}</string></array>
  <key>com.apple.security.application-groups</key><array><string>${GROUP}</string></array>
</dict></plist>
EOF
  if [ -d "$APP/Frameworks" ]; then
    for fw in "$APP"/Frameworks/*.framework; do
      [ -d "$fw" ] || continue; n="$(basename "$fw" .framework)"
      [ -f "$fw/$n" ] && ldid -S "$fw/$n"
    done
    for dy in "$APP"/Frameworks/*.dylib; do [ -f "$dy" ] && ldid -S "$dy"; done
  fi
  # Nested app extensions are separate binaries and need their own signature +
  # entitlements. An unsigned .appex still installs, then gets killed the moment
  # it runs — i.e. tapping Wisp in the share sheet just crashes. Sign before the
  # outer app.
  for ext in "$APP"/PlugIns/*.appex; do
    [ -d "$ext" ] || continue
    ext_bid="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleIdentifier' "$ext/Info.plist" 2>/dev/null || true)"
    case "$ext_bid" in ""|"(null)") ext_bid="${BID}.$(basename "$ext" .appex)";; esac
    ext_exe="$([ -x "$PB" ] && "$PB" -c 'Print :CFBundleExecutable' "$ext/Info.plist" 2>/dev/null || true)"
    case "$ext_exe" in ""|"(null)") ext_exe="$(basename "$ext" .appex)";; esac
    [ -f "$ext/$ext_exe" ] || continue
    echo "    signing extension $ext_bid"
    cat > ext.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>${ext_bid}</string>
  <key>com.apple.developer.team-identifier</key><string>0000000000</string>
  <key>get-task-allow</key><true/>
  <key>com.apple.security.application-groups</key><array><string>${GROUP}</string></array>
</dict></plist>
EOF
    ldid -S"$W/ext.plist" "$ext/$ext_exe"; chmod 0755 "$ext/$ext_exe"
  done
  ldid -S"$W/ent.plist" "$APP/$EXE"; chmod 0755 "$APP/$EXE"
  zip -qry /var/mobile/wisp-signed.ipa Payload
  echo "    signed.ipa -> /var/mobile/wisp-signed.ipa"
  echo "INSTALLED_VIA=none"
fi
REMOTE

echo
echo "==> If the line above says:"
echo "      INSTALLED_VIA=trollstore : done — launch the app on the phone."
echo "      INSTALLED_VIA=none       : open /var/mobile/wisp-signed.ipa in Filza -> Install"
echo "                                 (installd -> container -> Metal OK; do NOT drop it into"
echo "                                  /var/jb/Applications, that skips the container)."
