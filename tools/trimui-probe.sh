#!/bin/sh
# Dump everything wisp-trimui assumes about a TrimUI handheld.
#
# TrimUI has not published an SDK for the Brick Pro (TG4040), so the port was
# written against community reports rather than a device. Run this over SSH
# and send the output back; it confirms or corrects every assumption in one
# go.
#
#   scp tools/trimui-probe.sh root@<device>:/tmp/
#   ssh root@<device> 'sh /tmp/trimui-probe.sh' > probe.txt

section() {
    echo
    echo "===== $1 ====="
}

section "system"
uname -a
cat /etc/os-release 2>/dev/null || echo "(no /etc/os-release)"
echo "arch: $(uname -m)"

section "libc"
# Which dynamic loader exists tells us whether the rootfs is musl or glibc.
# The shipped binary is statically linked, so this is informational — it only
# matters if a dynamically linked build is ever attempted.
ls -l /lib/ld-musl-aarch64.so.1 2>/dev/null && echo "-> musl"
ls -l /lib/ld-linux-aarch64.so.1 /lib64/ld-linux-aarch64.so.1 2>/dev/null && echo "-> glibc"
(ldd --version 2>&1 | head -n 1) || true

section "cpu / memory"
grep -c ^processor /proc/cpuinfo
grep -E 'MemTotal|MemAvailable' /proc/meminfo

section "framebuffer"
# The app needs 32bpp and the exact stride; anything else and fb.rs bails.
for node in /dev/fb0 /dev/fb1; do
    [ -e "$node" ] && ls -l "$node"
done
for f in /sys/class/graphics/fb0/virtual_size \
         /sys/class/graphics/fb0/bits_per_pixel \
         /sys/class/graphics/fb0/stride \
         /sys/class/graphics/fb0/rotate; do
    [ -r "$f" ] && echo "$(basename "$f"): $(cat "$f")"
done
command -v fbset >/dev/null 2>&1 && fbset -i

section "input devices"
# Names and handlers here map to the /dev/input/eventN the app opens; the
# button tester in Settings reports the per-button codes.
cat /proc/bus/input/devices 2>/dev/null
ls -l /dev/input/ 2>/dev/null

section "storage"
df -h 2>/dev/null
for d in /mnt/SDCARD /mnt/UDISK /mnt/SDCARD/Apps /mnt/SDCARD/Roms; do
    [ -d "$d" ] && echo "exists: $d"
done

section "existing app manifests"
# Used to confirm the config.json schema the stock launcher actually reads.
for f in /mnt/SDCARD/Apps/*/config.json; do
    [ -r "$f" ] || continue
    echo "--- $f"
    cat "$f"
    echo
done

section "network"
ip addr 2>/dev/null || ifconfig 2>/dev/null
ip route 2>/dev/null || route -n 2>/dev/null
cat /etc/resolv.conf 2>/dev/null

section "launcher"
ps 2>/dev/null | grep -iE 'MainUI|runtrimui' | grep -v grep
ls -l /usr/trimui/bin 2>/dev/null | head -n 20

section "power management"
# A transfer that outlasts the idle timer dies with the Wi-Fi link, so the app
# has to hold the device awake while one is running. These are the handles it
# could use, in order of preference: an Android-style wake lock is the clean
# one (write a name to hold, write it again to /sys/power/wake_unlock to
# release), autosleep says whether anything is arming a suspend at all, and
# the backlight nodes say whether blanking is separate from suspending.
for f in /sys/power/state /sys/power/autosleep /sys/power/wake_lock          /sys/power/wakeup_count /sys/power/pm_async; do
    [ -e "$f" ] && echo "$f: $(cat "$f" 2>/dev/null)"
done
ls -l /sys/power/ 2>/dev/null
echo "--- wakeup sources (first 15):"
head -n 15 /sys/kernel/debug/wakeup_sources 2>/dev/null || echo "(not readable)"
echo "--- backlight:"
for d in /sys/class/backlight/*/; do
    [ -d "$d" ] || continue
    echo "$d brightness=$(cat "$d/brightness" 2>/dev/null) max=$(cat "$d/max_brightness" 2>/dev/null) bl_power=$(cat "$d/bl_power" 2>/dev/null)"
done
echo "--- fb blanking:"
cat /sys/class/graphics/fb0/blank 2>/dev/null
echo "--- who arms the idle timer (daemons still running under the app):"
ps 2>/dev/null | grep -viE 'grep|\[' | head -n 40
echo "--- trimui feature flags:"
for f in /usr/trimui/features.json /mnt/SDCARD/features.json /mnt/UDISK/system.json; do
    [ -r "$f" ] && { echo "--- $f"; cat "$f"; echo; }
done
echo "--- wifi power save:"
iw dev wlan0 get power_save 2>/dev/null || echo "(no iw)"

echo
echo "===== done ====="
