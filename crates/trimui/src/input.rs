//! Gamepad input via raw evdev.
//!
//! The TG4040 exposes its controls as a single event device that reports
//! itself as an Xbox 360 pad (045e:028e). The names are Xbox, but the layout is
//! Nintendo-style and the driver maps *physical position*, so the button the
//! user presses as "A" (right) is not necessarily `BTN_A`. That mapping cannot
//! be confirmed without the hardware, so it is a table here, overridable from
//! `config.json`, and the settings screen has a button tester that prints the
//! raw code for whatever is pressed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;

/// Delay before a held direction starts repeating.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
/// Interval between repeats once repeating has started.
const REPEAT_INTERVAL: Duration = Duration::from_millis(90);

/// Axis magnitude past which an analog stick counts as pushed.
const STICK_PRESS: i32 = 16_000;
/// Axis magnitude below which it counts as released. The gap between this and
/// [`STICK_PRESS`] is hysteresis, so a stick resting near the threshold does
/// not machine-gun the menu.
///
/// A TG4040 at rest was measured drifting to about 4 500 on X and -2 900 on Y,
/// and it emits those samples continuously, so the release threshold has to
/// stay comfortably above that or the menu scrolls on its own.
const STICK_RELEASE: i32 = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    Start,
    Select,
    Menu,
    Power,
}

impl Button {
    /// Directions auto-repeat when held; action buttons deliberately do not.
    fn repeats(self) -> bool {
        matches!(
            self,
            Button::Up | Button::Down | Button::Left | Button::Right
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Button::Up => "Up",
            Button::Down => "Down",
            Button::Left => "Left",
            Button::Right => "Right",
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::L1 => "L1",
            Button::R1 => "R1",
            Button::L2 => "L2",
            Button::R2 => "R2",
            Button::Start => "Start",
            Button::Select => "Select",
            Button::Menu => "Menu",
            Button::Power => "Power",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let all = [
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
            Button::A,
            Button::B,
            Button::X,
            Button::Y,
            Button::L1,
            Button::R1,
            Button::L2,
            Button::R2,
            Button::Start,
            Button::Select,
            Button::Menu,
            Button::Power,
        ];
        all.into_iter()
            .find(|b| b.label().eq_ignore_ascii_case(name))
    }
}

/// A button transition, plus the raw evdev code that produced it so the
/// button tester can show what the hardware actually sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub button: Option<Button>,
    pub pressed: bool,
    pub repeat: bool,
    pub raw_type: u16,
    pub raw_code: u16,
    pub raw_value: i32,
}

// evdev event types.
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;

// evdev axis codes.
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// Default evdev key code to [`Button`] mapping.
///
/// Covers the three spellings a TrimUI-class device may use: gamepad button
/// codes (`BTN_*`), D-pad button codes (`BTN_DPAD_*`), and plain keyboard
/// arrows, which some Allwinner firmwares emit for the D-pad.
pub fn default_key_map() -> HashMap<u16, Button> {
    let mut map = HashMap::new();
    // Face buttons, measured on a TG4040 by pressing each one in turn.
    //
    // The pad enumerates as an Xbox 360 controller, so the kernel names these
    // `BTN_A`(0x130), `BTN_B`(0x131), `BTN_X`(0x133) and `BTN_Y`(0x134) — but
    // the codes follow the *Xbox physical positions* while the handheld is
    // silkscreened Nintendo-style. The result is that the button labelled A
    // sends `BTN_B` and the one labelled X sends `BTN_Y`. Mapping by label,
    // not by kernel name, is what makes A confirm and B go back:
    map.insert(0x131, Button::A);
    map.insert(0x130, Button::B);
    map.insert(0x134, Button::X);
    map.insert(0x133, Button::Y);
    // Shoulders and triggers.
    map.insert(0x136, Button::L1);
    map.insert(0x137, Button::R1);
    map.insert(0x138, Button::L2);
    map.insert(0x139, Button::R2);
    // BTN_SELECT / BTN_START / BTN_MODE
    map.insert(0x13A, Button::Select);
    map.insert(0x13B, Button::Start);
    map.insert(0x13C, Button::Menu);
    // BTN_DPAD_UP..RIGHT
    map.insert(0x220, Button::Up);
    map.insert(0x221, Button::Down);
    map.insert(0x222, Button::Left);
    map.insert(0x223, Button::Right);
    // KEY_UP / KEY_LEFT / KEY_RIGHT / KEY_DOWN
    map.insert(103, Button::Up);
    map.insert(105, Button::Left);
    map.insert(106, Button::Right);
    map.insert(108, Button::Down);
    // KEY_POWER
    map.insert(116, Button::Power);
    map
}

/// Turns a stream of raw evdev records into debounced, auto-repeating
/// [`KeyEvent`]s. Split from the device reader so it can be unit tested
/// without `/dev/input`.
pub struct Decoder {
    key_map: HashMap<u16, Button>,
    /// Held buttons, with the deadline at which the next repeat fires.
    held: HashMap<Button, Instant>,
    /// Last emitted direction per analog axis, for hysteresis.
    axis_dir: HashMap<u16, i32>,
}

impl Decoder {
    pub fn new(key_map: HashMap<u16, Button>) -> Self {
        Self {
            key_map,
            held: HashMap::new(),
            axis_dir: HashMap::new(),
        }
    }

    /// Feed one raw evdev record. Returns the events it produced.
    pub fn feed(&mut self, now: Instant, kind: u16, code: u16, value: i32) -> Vec<KeyEvent> {
        match kind {
            EV_KEY => {
                // value 2 is the kernel's own key repeat; we generate our own
                // on a timer so the rate is consistent across devices.
                if value == 2 {
                    return Vec::new();
                }
                let pressed = value != 0;
                let button = self.key_map.get(&code).copied();
                if let Some(button) = button {
                    if pressed {
                        self.held.insert(button, now + REPEAT_DELAY);
                    } else {
                        self.held.remove(&button);
                    }
                }
                vec![KeyEvent {
                    button,
                    pressed,
                    repeat: false,
                    raw_type: kind,
                    raw_code: code,
                    raw_value: value,
                }]
            }
            EV_ABS => self.feed_axis(now, code, value),
            _ => Vec::new(),
        }
    }

    fn feed_axis(&mut self, now: Instant, code: u16, value: i32) -> Vec<KeyEvent> {
        let (negative, positive) = match code {
            ABS_HAT0X | ABS_X => (Button::Left, Button::Right),
            ABS_HAT0Y | ABS_Y => (Button::Up, Button::Down),
            _ => return Vec::new(),
        };

        // Hats report -1/0/1, sticks report a wide range. Normalising both to
        // -1/0/1 here lets one code path serve the D-pad and the stick.
        let is_hat = matches!(code, ABS_HAT0X | ABS_HAT0Y);
        let previous = self.axis_dir.get(&code).copied().unwrap_or(0);
        let direction = if is_hat {
            value.signum()
        } else if value >= STICK_PRESS {
            1
        } else if value <= -STICK_PRESS {
            -1
        } else if value.abs() <= STICK_RELEASE {
            0
        } else {
            previous
        };

        if direction == previous {
            return Vec::new();
        }
        self.axis_dir.insert(code, direction);

        let mut out = Vec::new();
        // Release whatever the axis was previously holding.
        match previous {
            -1 => {
                self.held.remove(&negative);
                out.push(KeyEvent {
                    button: Some(negative),
                    pressed: false,
                    repeat: false,
                    raw_type: EV_ABS,
                    raw_code: code,
                    raw_value: value,
                });
            }
            1 => {
                self.held.remove(&positive);
                out.push(KeyEvent {
                    button: Some(positive),
                    pressed: false,
                    repeat: false,
                    raw_type: EV_ABS,
                    raw_code: code,
                    raw_value: value,
                });
            }
            _ => {}
        }
        let now_button = match direction {
            -1 => Some(negative),
            1 => Some(positive),
            _ => None,
        };
        if let Some(button) = now_button {
            self.held.insert(button, now + REPEAT_DELAY);
            out.push(KeyEvent {
                button: Some(button),
                pressed: true,
                repeat: false,
                raw_type: EV_ABS,
                raw_code: code,
                raw_value: value,
            });
        }
        out
    }

    /// Emit auto-repeats for directions held past their deadline. Called once
    /// per frame, independently of whether the device sent anything.
    pub fn tick(&mut self, now: Instant) -> Vec<KeyEvent> {
        let mut out = Vec::new();
        for (button, deadline) in self.held.iter_mut() {
            if !button.repeats() || now < *deadline {
                continue;
            }
            *deadline = now + REPEAT_INTERVAL;
            out.push(KeyEvent {
                button: Some(*button),
                pressed: true,
                repeat: true,
                raw_type: 0,
                raw_code: 0,
                raw_value: 0,
            });
        }
        out
    }

    /// Drops all held state, so a screen change cannot inherit a stuck repeat.
    pub fn clear(&mut self) {
        self.held.clear();
        self.axis_dir.clear();
    }
}

#[cfg(target_os = "linux")]
pub use self::linux::Devices;

#[cfg(not(target_os = "linux"))]
pub use self::stub::Devices;

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    use anyhow::{Context, Result, bail};

    /// Byte offset of `type` within `struct input_event`, i.e. the size of the
    /// leading `struct timeval`. Derived rather than hardcoded because that
    /// width is what differs between ABIs.
    const TIME_SIZE: usize = std::mem::size_of::<libc::timeval>();

    /// One raw evdev record: a `timeval`, then `__u16 type`, `__u16 code` and
    /// `__s32 value` — 24 bytes on 64-bit Linux.
    const EVENT_SIZE: usize = TIME_SIZE + 8;

    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(EVENT_SIZE == 24);

    pub struct Devices {
        files: Vec<File>,
        buffer: Vec<u8>,
    }

    impl Devices {
        /// Opens every readable `/dev/input/event*`. Opening them all avoids
        /// having to guess which node is the pad — the TG4040 reports it as
        /// `event3`, but that is not guaranteed across firmware versions.
        pub fn open_all() -> Result<Self> {
            let mut files = Vec::new();
            let dir = std::fs::read_dir("/dev/input").context("read /dev/input")?;
            let mut paths: Vec<_> = dir
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("event"))
                })
                .collect();
            paths.sort();

            for path in paths {
                match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&path)
                {
                    Ok(file) => {
                        tracing::debug!(
                            target: "wisp_trimui::input",
                            path = %path.display(),
                            "opened input device"
                        );
                        files.push(file);
                    }
                    Err(err) => {
                        tracing::debug!(
                            target: "wisp_trimui::input",
                            path = %path.display(),
                            error = %err,
                            "skipping input device"
                        );
                    }
                }
            }

            if files.is_empty() {
                bail!("no readable device under /dev/input — is this running as root?");
            }
            Ok(Self {
                files,
                buffer: vec![0u8; EVENT_SIZE * 64],
            })
        }

        /// Non-blocking drain of every device. Returns `(type, code, value)`.
        pub fn read_raw(&mut self) -> Vec<(u16, u16, i32)> {
            let mut out = Vec::new();
            for file in &self.files {
                loop {
                    let read = unsafe {
                        libc::read(
                            file.as_raw_fd(),
                            self.buffer.as_mut_ptr().cast::<libc::c_void>(),
                            self.buffer.len(),
                        )
                    };
                    if read <= 0 {
                        break;
                    }
                    let read = read as usize;
                    for chunk in self.buffer[..read].chunks_exact(EVENT_SIZE) {
                        let kind = u16::from_ne_bytes([chunk[TIME_SIZE], chunk[TIME_SIZE + 1]]);
                        let code = u16::from_ne_bytes([chunk[TIME_SIZE + 2], chunk[TIME_SIZE + 3]]);
                        let value = i32::from_ne_bytes([
                            chunk[TIME_SIZE + 4],
                            chunk[TIME_SIZE + 5],
                            chunk[TIME_SIZE + 6],
                            chunk[TIME_SIZE + 7],
                        ]);
                        out.push((kind, code, value));
                    }
                    if read < self.buffer.len() {
                        break;
                    }
                }
            }
            out
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod stub {
    use anyhow::{Result, bail};

    pub struct Devices {
        _private: (),
    }

    impl Devices {
        pub fn open_all() -> Result<Self> {
            bail!("the evdev backend is only available on Linux")
        }

        pub fn read_raw(&mut self) -> Vec<(u16, u16, i32)> {
            Vec::new()
        }
    }
}

/// Reads the device and decodes it, the pairing the app loop actually uses.
pub struct Input {
    devices: Devices,
    decoder: Decoder,
}

impl Input {
    pub fn open(key_map: HashMap<u16, Button>) -> Result<Self> {
        Ok(Self {
            devices: Devices::open_all()?,
            decoder: Decoder::new(key_map),
        })
    }

    pub fn poll(&mut self) -> Vec<KeyEvent> {
        let now = Instant::now();
        let mut out = Vec::new();
        for (kind, code, value) in self.devices.read_raw() {
            out.extend(self.decoder.feed(now, kind, code, value));
        }
        out.extend(self.decoder.tick(now));
        out
    }

    pub fn clear(&mut self) {
        self.decoder.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        // A fixed base keeps the arithmetic obvious; `Instant` has no public
        // constructor, so derive every timestamp from one `now`.
        *TEST_BASE + Duration::from_millis(ms)
    }

    static TEST_BASE: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

    #[test]
    fn key_press_and_release_map_to_buttons() {
        let mut decoder = Decoder::new(default_key_map());
        let pressed = decoder.feed(at(0), EV_KEY, 0x131, 1);
        assert_eq!(pressed.len(), 1);
        assert_eq!(pressed[0].button, Some(Button::A));
        assert!(pressed[0].pressed);

        let released = decoder.feed(at(10), EV_KEY, 0x131, 0);
        assert_eq!(released[0].button, Some(Button::A));
        assert!(!released[0].pressed);
    }

    /// Locks in the measured TG4040 layout: the kernel's `BTN_A` is the
    /// physical B, and `BTN_X` is the physical Y. Getting this backwards
    /// swaps confirm and cancel, so it is worth a test of its own.
    #[test]
    fn face_buttons_follow_the_silkscreen_not_the_kernel_names() {
        let map = default_key_map();
        assert_eq!(map.get(&0x131), Some(&Button::A), "BTN_B is the A button");
        assert_eq!(map.get(&0x130), Some(&Button::B), "BTN_A is the B button");
        assert_eq!(map.get(&0x134), Some(&Button::X), "BTN_Y is the X button");
        assert_eq!(map.get(&0x133), Some(&Button::Y), "BTN_X is the Y button");
    }

    /// The TG4040 reports its D-pad on the hat axes, with negative meaning up
    /// and left — confirmed on hardware.
    #[test]
    fn the_hat_axes_carry_the_dpad() {
        let mut decoder = Decoder::new(default_key_map());
        assert_eq!(
            decoder.feed(at(0), EV_ABS, ABS_HAT0Y, -1)[0].button,
            Some(Button::Up)
        );
        decoder.clear();
        assert_eq!(
            decoder.feed(at(10), EV_ABS, ABS_HAT0X, -1)[0].button,
            Some(Button::Left)
        );
    }

    #[test]
    fn unknown_codes_still_surface_for_the_button_tester() {
        let mut decoder = Decoder::new(default_key_map());
        let events = decoder.feed(at(0), EV_KEY, 0x2FF, 1);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].button, None);
        assert_eq!(events[0].raw_code, 0x2FF);
    }

    #[test]
    fn kernel_autorepeat_is_ignored() {
        let mut decoder = Decoder::new(default_key_map());
        assert!(decoder.feed(at(0), EV_KEY, 0x130, 2).is_empty());
    }

    #[test]
    fn hat_axis_presses_then_releases_a_direction() {
        let mut decoder = Decoder::new(default_key_map());
        let down = decoder.feed(at(0), EV_ABS, ABS_HAT0Y, 1);
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].button, Some(Button::Down));
        assert!(down[0].pressed);

        let centre = decoder.feed(at(10), EV_ABS, ABS_HAT0Y, 0);
        assert_eq!(centre.len(), 1);
        assert_eq!(centre[0].button, Some(Button::Down));
        assert!(!centre[0].pressed);
    }

    #[test]
    fn stick_uses_hysteresis_between_press_and_release() {
        let mut decoder = Decoder::new(default_key_map());
        assert!(decoder.feed(at(0), EV_ABS, ABS_X, 12_000).is_empty());
        let pressed = decoder.feed(at(10), EV_ABS, ABS_X, 20_000);
        assert_eq!(pressed[0].button, Some(Button::Right));
        // Still above the release threshold, so it stays held.
        assert!(decoder.feed(at(20), EV_ABS, ABS_X, 12_000).is_empty());
        let released = decoder.feed(at(30), EV_ABS, ABS_X, 1_000);
        assert_eq!(released[0].button, Some(Button::Right));
        assert!(!released[0].pressed);
    }

    #[test]
    fn directions_repeat_after_the_delay_but_actions_do_not() {
        let mut decoder = Decoder::new(default_key_map());
        decoder.feed(at(0), EV_KEY, 0x220, 1);
        decoder.feed(at(0), EV_KEY, 0x130, 1);

        assert!(decoder.tick(at(100)).is_empty(), "too early to repeat");

        let repeats = decoder.tick(at(500));
        assert_eq!(repeats.len(), 1, "only the direction repeats");
        assert_eq!(repeats[0].button, Some(Button::Up));
        assert!(repeats[0].repeat);
    }

    #[test]
    fn clear_drops_held_repeats() {
        let mut decoder = Decoder::new(default_key_map());
        decoder.feed(at(0), EV_KEY, 0x220, 1);
        decoder.clear();
        assert!(decoder.tick(at(1_000)).is_empty());
    }
}
