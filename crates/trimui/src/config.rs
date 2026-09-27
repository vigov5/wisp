//! Persisted settings.
//!
//! Everything lives in one JSON file under the state directory so a user can
//! edit it over SSH, and so wiping it is a single `rm`. The device identity is
//! part of it on purpose: senders remember a receiver by its public key, and a
//! key regenerated per launch would make every saved entry on the sending side
//! go stale after one reboot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::input::Button;

/// Where received files land unless the user picks otherwise.
pub const DEFAULT_SAVE_ROOT: &str = "/mnt/SDCARD/Wisp";
/// Hidden, so it does not show up as a folder in the stock file browser.
pub const DEFAULT_STATE_DIR: &str = "/mnt/SDCARD/.wisp";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    /// The sender's iroh endpoint id (its public key), which is what makes
    /// this a trust decision about an identity rather than about a name.
    pub endpoint_id: String,
    pub name: String,
}

/// How to resolve a name that already exists in the save folder.
///
/// `Overwrite` is deliberately absent: `ReceiverService::start` rejects it, so
/// offering it in the settings screen would only produce a receiver that
/// refuses to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Conflict {
    Rename,
    Reject,
}

impl Conflict {
    pub fn as_str(self) -> &'static str {
        match self {
            Conflict::Rename => "rename",
            Conflict::Reject => "reject",
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        let strings = lang.strings();
        match self {
            Conflict::Rename => strings.conflict_rename,
            Conflict::Reject => strings.conflict_reject,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Conflict::Rename => Conflict::Reject,
            Conflict::Reject => Conflict::Rename,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub device_name: String,
    /// UI language. Defaults to English; Vietnamese is one row away in
    /// Settings.
    pub lang: Lang,
    pub save_root: PathBuf,
    pub conflict: Conflict,
    /// Rendezvous base URL. `None` uses the build's default server.
    pub server: Option<String>,
    /// Auto-accept offers from these senders (feature R6).
    pub trusted: Vec<TrustedDevice>,
    /// Hex-encoded 32-byte iroh secret key; generated on first run.
    pub secret_key: Option<String>,
    /// Raw evdev code -> button name, for devices whose layout does not match
    /// [`crate::input::default_key_map`]. The button tester in the settings
    /// screen exists to fill this in.
    pub button_overrides: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            lang: Lang::default(),
            save_root: PathBuf::from(DEFAULT_SAVE_ROOT),
            conflict: Conflict::Rename,
            server: None,
            trusted: Vec::new(),
            secret_key: None,
            button_overrides: HashMap::new(),
        }
    }
}

/// The name senders see. The hostname is usually `TinaLinux` on stock
/// firmware, which tells the user nothing, so fall back to the product name.
fn default_device_name() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("tinalinux"));
    host.unwrap_or_else(|| "TrimUI Brick Pro".to_owned())
}

impl Config {
    pub fn state_dir() -> PathBuf {
        std::env::var("WISP_TRIMUI_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_STATE_DIR))
    }

    pub fn path() -> PathBuf {
        Self::state_dir().join("settings.json")
    }

    /// Loads the config, falling back to defaults when the file is missing or
    /// unreadable. A corrupt file must not brick the app — the user has no
    /// keyboard to fix it with — so a parse failure is logged and replaced.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Config>(&text) {
                Ok(config) => config,
                Err(err) => {
                    tracing::warn!(
                        target: "wisp_trimui::config",
                        path = %path.display(),
                        error = %err,
                        "settings file is unreadable; using defaults"
                    );
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create state dir {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("serialize settings")?;
        // Write-then-rename: a power cut mid-write on a handheld is a real
        // event, and a truncated settings.json would lose the identity key.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, &text).with_context(|| format!("write {}", temp.display()))?;
        std::fs::rename(&temp, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }

    /// The device's stable iroh identity, generated on first run.
    ///
    /// Returns the key and whether the config changed, so the caller can
    /// decide when to write the file rather than having this method do IO.
    pub fn ensure_secret_key(&mut self) -> (iroh::SecretKey, bool) {
        if let Some(stored) = self.secret_key.as_deref() {
            match decode_hex32(stored) {
                Some(bytes) => return (iroh::SecretKey::from_bytes(&bytes), false),
                None => tracing::warn!(
                    target: "wisp_trimui::config",
                    "stored identity key is malformed; generating a new one"
                ),
            }
        }
        let bytes: [u8; 32] = rand::random();
        self.secret_key = Some(encode_hex(&bytes));
        (iroh::SecretKey::from_bytes(&bytes), true)
    }

    pub fn is_trusted(&self, endpoint_id: &str) -> bool {
        !endpoint_id.is_empty()
            && self
                .trusted
                .iter()
                .any(|device| device.endpoint_id == endpoint_id)
    }

    /// Adds a device to the trust list, replacing any existing entry for the
    /// same key so the stored name follows the sender's current one.
    pub fn trust(&mut self, endpoint_id: &str, name: &str) {
        if endpoint_id.is_empty() {
            return;
        }
        self.trusted
            .retain(|device| device.endpoint_id != endpoint_id);
        self.trusted.push(TrustedDevice {
            endpoint_id: endpoint_id.to_owned(),
            name: name.to_owned(),
        });
    }

    pub fn untrust(&mut self, endpoint_id: &str) {
        self.trusted
            .retain(|device| device.endpoint_id != endpoint_id);
    }

    /// The effective evdev mapping: defaults, with the user's overrides on top.
    pub fn key_map(&self) -> HashMap<u16, Button> {
        let mut map = crate::input::default_key_map();
        for (code, name) in &self.button_overrides {
            match (code.parse::<u16>(), Button::from_name(name)) {
                (Ok(code), Some(button)) => {
                    map.insert(code, button);
                }
                _ => tracing::warn!(
                    target: "wisp_trimui::config",
                    code, name, "ignoring unusable button override"
                ),
            }
        }
        map
    }

    /// Candidate save folders offered by the settings screen, before the
    /// directory browser. Only existing directories are listed, so the menu
    /// does not advertise a Roms folder on a card that has none.
    pub fn save_root_suggestions() -> Vec<PathBuf> {
        let candidates = [
            DEFAULT_SAVE_ROOT,
            "/mnt/SDCARD/Roms",
            "/mnt/SDCARD/Screenshots",
            "/mnt/SDCARD/Music",
            "/mnt/SDCARD",
        ];
        let mut out: Vec<PathBuf> = Vec::new();
        for candidate in candidates {
            let path = Path::new(candidate);
            // The default is always offered: it is created on demand.
            if candidate == DEFAULT_SAVE_ROOT || path.is_dir() {
                out.push(path.to_path_buf());
            }
        }
        out
    }
}

/// The mount point currently providing `path`, parsed from `/proc/mounts`
/// content.
///
/// Returns the longest mount point that is a prefix of `path`, so
/// `/mnt/SDCARD/Wisp` resolves to `/mnt/SDCARD` when the card is mounted and
/// to `/` when it is not.
pub fn providing_mount(path: &Path, mounts: &str) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    for line in mounts.lines() {
        // `<device> <mount point> <type> ...`, with octal escapes in the path.
        let Some(point) = line.split_whitespace().nth(1) else {
            continue;
        };
        let point = PathBuf::from(point.replace("\\040", " "));
        if !path.starts_with(&point) {
            continue;
        }
        let better = best
            .as_ref()
            .is_none_or(|current| point.as_os_str().len() > current.as_os_str().len());
        if better {
            best = Some(point);
        }
    }
    best
}

/// Whether it is safe to write received files to `save_root`.
///
/// On this handheld the save folder lives on the SD card, and the card is a
/// mount point. When it fails to mount, `/mnt/SDCARD` does not disappear — it
/// quietly becomes an ordinary directory on the internal overlay. Anything
/// written there lands on internal storage and then vanishes from view the
/// moment the card mounts again and shadows it. A received file can be
/// transferred, verified and reported as saved, and still be nowhere the user
/// can find it.
///
/// So a path under `/mnt` is only accepted when something is actually mounted
/// at or above it. Paths elsewhere (a desktop run, a test) are left alone.
pub fn save_root_is_writable(save_root: &Path, mounts: &str) -> bool {
    if !save_root.starts_with("/mnt") {
        return true;
    }
    match providing_mount(save_root, mounts) {
        Some(point) => point != Path::new("/"),
        None => false,
    }
}

/// Reads `/proc/mounts`, or an empty string when it cannot be read.
pub fn read_mounts() -> String {
    std::fs::read_to_string("/proc/mounts").unwrap_or_default()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Parses exactly 32 bytes of hex, rejecting anything else. A short or
/// mistyped key must produce a fresh identity rather than a panic.
fn decode_hex32(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable_without_a_file() {
        let config = Config::default();
        assert_eq!(config.conflict, Conflict::Rename);
        assert!(config.trusted.is_empty());
        assert!(!config.device_name.is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let mut config = Config::default();
        config.trust("abc123", "Pixel 7");
        config.conflict = Conflict::Reject;
        config.secret_key = Some("00".repeat(32));

        let text = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Forward compatibility: an older settings.json must still load.
        let parsed: Config = serde_json::from_str(r#"{"device_name":"Brick"}"#).unwrap();
        assert_eq!(parsed.device_name, "Brick");
        assert_eq!(parsed.conflict, Conflict::Rename);
        assert_eq!(parsed.save_root, PathBuf::from(DEFAULT_SAVE_ROOT));
    }

    #[test]
    fn trust_is_keyed_by_endpoint_and_does_not_duplicate() {
        let mut config = Config::default();
        config.trust("key-1", "Old name");
        config.trust("key-1", "New name");
        assert_eq!(config.trusted.len(), 1);
        assert_eq!(config.trusted[0].name, "New name");
        assert!(config.is_trusted("key-1"));

        config.untrust("key-1");
        assert!(!config.is_trusted("key-1"));
    }

    #[test]
    fn an_empty_endpoint_id_is_never_trusted() {
        let mut config = Config::default();
        // Ephemeral CLI/browser senders can arrive without a stable id; those
        // must never silently match the trust list.
        config.trust("", "nobody");
        assert!(config.trusted.is_empty());
        assert!(!config.is_trusted(""));
    }

    #[test]
    fn button_overrides_replace_the_default_mapping() {
        let mut config = Config::default();
        // 305 is the A button by default on a TG4040; overriding it proves the
        // user's table wins over the built-in one.
        config
            .button_overrides
            .insert("305".to_owned(), "Start".to_owned());
        let map = config.key_map();
        assert_eq!(map.get(&305), Some(&Button::Start));
    }

    #[test]
    fn unusable_button_overrides_are_ignored() {
        let mut config = Config::default();
        config
            .button_overrides
            .insert("not-a-code".to_owned(), "A".to_owned());
        config
            .button_overrides
            .insert("306".to_owned(), "NotAButton".to_owned());
        let map = config.key_map();
        assert_eq!(map.get(&306), None);
        // The defaults survive a bad override.
        assert_eq!(map.get(&0x131), Some(&Button::A));
    }

    #[test]
    fn the_identity_key_is_generated_once_then_reused() {
        let mut config = Config::default();
        let (first, changed) = config.ensure_secret_key();
        assert!(changed, "a fresh config has no key yet");

        let (second, changed_again) = config.ensure_secret_key();
        assert!(!changed_again, "the stored key must be reused");
        assert_eq!(
            first.to_bytes(),
            second.to_bytes(),
            "senders remember this receiver by its key; it must survive a restart"
        );
    }

    #[test]
    fn a_malformed_identity_key_is_replaced_rather_than_fatal() {
        let mut config = Config::default();
        config.secret_key = Some("nonsense".to_owned());
        let (_key, changed) = config.ensure_secret_key();
        assert!(changed);
        assert_eq!(config.secret_key.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 15, 16, 127, 128, 255, 42, 9, 200].repeat(4);
        let encoded = encode_hex(&bytes[..32]);
        assert_eq!(encoded.len(), 64);
        assert_eq!(
            decode_hex32(&encoded),
            Some(bytes[..32].try_into().unwrap())
        );
    }

    #[test]
    fn hex_decoding_rejects_bad_input() {
        assert_eq!(decode_hex32(""), None);
        assert_eq!(decode_hex32(&"0".repeat(63)), None);
        assert_eq!(decode_hex32(&"z".repeat(64)), None);
    }

    #[test]
    fn conflict_labels_and_toggle_stay_in_sync() {
        assert_eq!(Conflict::Rename.toggled(), Conflict::Reject);
        assert_eq!(Conflict::Reject.toggled(), Conflict::Rename);
        assert_eq!(Conflict::Rename.as_str(), "rename");
        assert_eq!(Conflict::Reject.label(Lang::En), "Reject");
        assert_eq!(Conflict::Reject.label(Lang::Vi), "Từ chối");
    }

    /// The real `/proc/mounts` from a TG4040 with the card mounted.
    const MOUNTS_WITH_CARD: &str = "\
/dev/root /rom squashfs ro,relatime 0 0
/dev/by-name/rootfs_data /overlay ext4 rw,sync,relatime 0 0
overlayfs:/overlay / overlay rw,noatime,lowerdir=/,upperdir=/overlay/upper 0 0
/dev/mmcblk1p1 /mnt/SDCARD vfat rw,sync,relatime 0 0
/dev/by-name/UDISK /mnt/UDISK ext4 rw,sync,relatime 0 0
";

    /// The same device with the card missing or busy — note `/mnt/SDCARD` is
    /// simply absent, not reported as an error.
    const MOUNTS_WITHOUT_CARD: &str = "\
/dev/root /rom squashfs ro,relatime 0 0
/dev/by-name/rootfs_data /overlay ext4 rw,sync,relatime 0 0
overlayfs:/overlay / overlay rw,noatime,lowerdir=/,upperdir=/overlay/upper 0 0
/dev/by-name/UDISK /mnt/UDISK ext4 rw,sync,relatime 0 0
";

    #[test]
    fn providing_mount_picks_the_longest_match() {
        assert_eq!(
            providing_mount(Path::new("/mnt/SDCARD/Wisp"), MOUNTS_WITH_CARD),
            Some(PathBuf::from("/mnt/SDCARD"))
        );
    }

    #[test]
    fn an_unmounted_card_falls_back_to_the_root_filesystem() {
        assert_eq!(
            providing_mount(Path::new("/mnt/SDCARD/Wisp"), MOUNTS_WITHOUT_CARD),
            Some(PathBuf::from("/")),
        );
    }

    #[test]
    fn the_save_root_is_refused_when_the_card_is_not_mounted() {
        // This is the whole point: writing here would put received files on
        // internal storage, where the card hides them again on the next mount.
        assert!(save_root_is_writable(
            Path::new("/mnt/SDCARD/Wisp"),
            MOUNTS_WITH_CARD
        ));
        assert!(!save_root_is_writable(
            Path::new("/mnt/SDCARD/Wisp"),
            MOUNTS_WITHOUT_CARD
        ));
    }

    #[test]
    fn a_save_root_on_internal_storage_is_still_accepted() {
        // /mnt/UDISK is genuinely mounted, so it is a legitimate choice.
        assert!(save_root_is_writable(
            Path::new("/mnt/UDISK/Wisp"),
            MOUNTS_WITHOUT_CARD
        ));
    }

    #[test]
    fn paths_outside_mnt_are_not_second_guessed() {
        // Desktop runs and tests must not be blocked by a device-specific rule.
        assert!(save_root_is_writable(Path::new("/tmp/out"), ""));
        assert!(save_root_is_writable(Path::new("downloads"), ""));
    }

    #[test]
    fn an_unreadable_mounts_file_refuses_a_mnt_path() {
        // Better to stop than to guess, when the destination is the card.
        assert!(!save_root_is_writable(Path::new("/mnt/SDCARD/Wisp"), ""));
    }

    #[test]
    fn a_fresh_config_is_english() {
        assert_eq!(Config::default().lang, Lang::En);
    }

    #[test]
    fn the_language_choice_persists() {
        let mut config = Config::default();
        config.lang = Lang::Vi;
        let text = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.lang, Lang::Vi);
    }

    #[test]
    fn an_older_settings_file_without_a_language_loads_as_english() {
        let parsed: Config = serde_json::from_str(r#"{"device_name":"Brick"}"#).unwrap();
        assert_eq!(parsed.lang, Lang::En);
    }
}
