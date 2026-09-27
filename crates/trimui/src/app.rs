//! Screen state machine.
//!
//! The app never touches the framebuffer or the receiver directly: it consumes
//! [`EngineEvent`]s and button presses, and answers with [`AppRequest`]s that
//! `main` carries out. That keeps every interaction — including auto-accept,
//! which is a trust decision — reachable from a unit test.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use wisp_app::{
    AcceptedDestinations, NearbyReceiver, OfferDecision, PairingCodeState, QrPairingInfo,
    ReceiverOfferEvent, ReceiverOfferPhase, SendEvent, SendPhase,
};
use wisp_core::util::human_size;

use crate::config::Config;
use crate::draw::{Canvas, Rect};
use crate::engine::{EngineCommand, SendTarget};
use crate::font::{Align, Fonts, Weight};
use crate::i18n::{Strings, fill, fill2, plural};
use crate::input::{Button, KeyEvent};
use crate::qr::QrImage;
use crate::theme::{self, metrics, text};
use crate::ui::{self, Row};

/// How long a toast stays on screen.
const TOAST_LIFETIME: Duration = Duration::from_secs(3);

/// How long the destination picker browses the LAN for receivers.
const NEARBY_SCAN_SECS: u64 = 6;

/// Rendezvous codes are six characters.
const CODE_LENGTH: usize = 6;

/// Keys per row on the on-screen keyboard. Nine keeps the grid four rows
/// deep, so every character is at most a few presses away.
const CODE_COLUMNS: usize = 9;

/// The on-screen keyboard's alphabet. Codes are upper-case alphanumeric, and
/// the input is the only text this app ever has to type.
const CODE_KEYS: [char; 36] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', //
    '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', //
    'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', //
    'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', //
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Idle: pairing code, QR, and where files will land.
    Home,
    /// An incoming transfer is waiting for a decision.
    Offer,
    /// Connecting or receiving.
    Transfer,
    /// Terminal state of the last transfer, including received text.
    Result,
    Settings,
    SaveFolder,
    Trusted,
    ButtonTest,
    About,
    /// Choosing what to send.
    SendPick,
    /// Choosing where to send it.
    SendTo,
    /// Typing a six-character pairing code.
    SendCode,
    SendProgress,
    SendResult,
}

/// What the app asks `main` to do after handling an input or event.
#[derive(Debug, Clone)]
pub enum AppRequest {
    Engine(EngineCommand),
    /// The save folder or conflict policy changed; the receiver has to be
    /// rebuilt because both are fixed at `ReceiverService::start`.
    RestartEngine,
    SaveConfig,
    Exit,
}

#[derive(Debug, Clone)]
struct Toast {
    message: String,
    until: Instant,
}

/// The settings menu, as one list.
///
/// Rows and their actions are driven from [`SettingsRow::ORDER`] rather than
/// from parallel `match index` arms: inserting a row used to mean remembering
/// to renumber the handler, and forgetting would silently wire a row to the
/// wrong action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsRow {
    SaveFolder,
    Conflict,
    Language,
    Trusted,
    ButtonTest,
    DeviceName,
    About,
}

impl SettingsRow {
    const ORDER: [SettingsRow; 7] = [
        SettingsRow::SaveFolder,
        SettingsRow::Conflict,
        SettingsRow::Language,
        SettingsRow::Trusted,
        SettingsRow::ButtonTest,
        SettingsRow::DeviceName,
        SettingsRow::About,
    ];

    fn at(index: usize) -> Option<Self> {
        Self::ORDER.get(index).copied()
    }

    fn render(self, app: &App) -> Row {
        let strings = app.s();
        match self {
            SettingsRow::SaveFolder => Row::new(strings.row_save_folder)
                .with_subtitle(app.config.save_root.display().to_string())
                .with_value("›"),
            SettingsRow::Conflict => Row::new(strings.row_name_clash)
                .with_value(app.config.conflict.label(app.config.lang)),
            SettingsRow::Language => {
                Row::new(strings.row_language).with_value(app.config.lang.label())
            }
            SettingsRow::Trusted => {
                Row::new(strings.row_trusted).with_value(format!("{}", app.config.trusted.len()))
            }
            SettingsRow::ButtonTest => Row::new(strings.row_button_test).with_value("›"),
            SettingsRow::DeviceName => Row::new(strings.row_device_name)
                .with_subtitle(strings.row_device_name_hint)
                .with_value(app.config.device_name.clone()),
            SettingsRow::About => Row::new(strings.row_about).with_value(env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Cursor and scroll position for one list.
#[derive(Debug, Clone, Default)]
struct Cursor {
    index: usize,
    scroll: usize,
}

impl Cursor {
    fn move_by(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.index = 0;
            return;
        }
        let last = count - 1;
        self.index = match delta {
            d if d < 0 => self.index.saturating_sub(d.unsigned_abs()),
            d => (self.index + d as usize).min(last),
        };
    }

    fn clamp(&mut self, count: usize) {
        if count == 0 {
            self.index = 0;
        } else {
            self.index = self.index.min(count - 1);
        }
    }
}

/// Filesystem browser, used for both pickers.
///
/// The save-folder picker lists directories only — offering files there would
/// present choices that cannot be selected. The send picker lists both.
#[derive(Debug, Clone)]
struct Browser {
    dir: PathBuf,
    entries: Vec<PathBuf>,
    cursor: Cursor,
    include_files: bool,
}

impl Browser {
    fn open(dir: PathBuf, include_files: bool) -> Self {
        let entries = Self::read_entries(&dir, include_files);
        Self {
            dir,
            entries,
            cursor: Cursor::default(),
            include_files,
        }
    }

    /// Directories first, then files, each sorted by name — the order a file
    /// manager uses, and the one that keeps a folder's subfolders together
    /// rather than interleaved with its contents.
    fn read_entries(dir: &Path, include_files: bool) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            // Hidden entries are noise here, and `.wisp` is our own state.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if include_files {
                files.push(path);
            }
        }
        dirs.sort();
        files.sort();
        dirs.extend(files);
        dirs
    }

    fn enter(&mut self, path: PathBuf) {
        self.dir = path;
        self.entries = Self::read_entries(&self.dir, self.include_files);
        self.cursor = Cursor::default();
    }

    fn up(&mut self) -> bool {
        match self.dir.parent().map(Path::to_path_buf) {
            Some(parent) => {
                self.enter(parent);
                true
            }
            None => false,
        }
    }
}

pub struct App {
    pub config: Config,
    screen: Screen,
    /// Where `B` returns to from the settings sub-screens.
    ready: bool,
    /// This device's endpoint id (public key), once the receiver is up.
    endpoint_id: Option<String>,
    code: PairingCodeState,
    code_error: Option<String>,
    pairing: Option<QrPairingInfo>,
    qr: Option<QrImage>,
    offer: Option<ReceiverOfferEvent>,
    /// Set once the current offer has been auto-accepted, so a repeated
    /// `OfferReady` cannot answer twice.
    auto_accepted: bool,
    fatal: Option<String>,
    toast: Option<Toast>,

    settings_cursor: Cursor,
    folder_cursor: Cursor,
    trusted_cursor: Cursor,
    offer_scroll: usize,
    text_scroll: usize,
    browser: Option<Browser>,
    last_key: Option<KeyEvent>,

    // --- sending
    send_browser: Option<Browser>,
    /// Paths queued for the next send, in the order they were picked.
    send_selection: Vec<PathBuf>,
    send_pick_cursor: Cursor,
    send_to_cursor: Cursor,
    /// `None` while a scan is in flight; `Some(Err(_))` when it failed.
    nearby: Option<Result<Vec<NearbyReceiver>, String>>,
    /// The code being typed, and where the on-screen keyboard's cursor sits.
    code_input: String,
    code_key: usize,
    send_event: Option<SendEvent>,
    /// Set once the finished send has been written to the recent list, so a
    /// repeated terminal event cannot record it twice.
    send_recorded: bool,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            screen: Screen::Home,
            ready: false,
            endpoint_id: None,
            code: PairingCodeState::Unavailable,
            code_error: None,
            pairing: None,
            qr: None,
            offer: None,
            auto_accepted: false,
            fatal: None,
            toast: None,
            settings_cursor: Cursor::default(),
            folder_cursor: Cursor::default(),
            trusted_cursor: Cursor::default(),
            offer_scroll: 0,
            text_scroll: 0,
            browser: None,
            last_key: None,
            send_browser: None,
            send_selection: Vec::new(),
            send_pick_cursor: Cursor::default(),
            send_to_cursor: Cursor::default(),
            nearby: None,
            code_input: String::new(),
            code_key: 0,
            send_event: None,
            send_recorded: false,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Jumps straight to a send screen with `selection` already queued.
    ///
    /// Only for `examples/preview.rs`: rendering the send flow off-device
    /// otherwise means walking a file picker that has nothing to walk. The
    /// app itself never calls this.
    pub fn preview_send(&mut self, screen: Screen, selection: Vec<PathBuf>) {
        self.send_selection = selection;
        self.screen = screen;
    }

    /// Puts the app into the "cannot run" state before the loop starts, used
    /// when startup finds a problem the receiver itself never sees — such as
    /// a save folder on an unmounted card.
    pub fn set_fatal(&mut self, message: impl Into<String>) {
        self.fatal = Some(message.into());
    }

    /// UI copy for the selected language. Returns `&'static` deliberately, so
    /// it holds no borrow on `self` and can be used inside `&mut self` render
    /// methods.
    fn s(&self) -> &'static Strings {
        self.config.lang.strings()
    }

    fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some(Toast {
            message: message.into(),
            until: Instant::now() + TOAST_LIFETIME,
        });
    }

    // ---------------------------------------------------------------- events

    pub fn handle_engine(&mut self, event: crate::engine::EngineEvent) -> Vec<AppRequest> {
        use crate::engine::EngineEvent as E;
        match event {
            E::Ready { endpoint_id } => {
                self.ready = true;
                self.endpoint_id = Some(endpoint_id);
                Vec::new()
            }
            E::Code(state) => {
                self.code = state;
                // A code arriving means the rendezvous call worked after all.
                self.code_error = None;
                Vec::new()
            }
            E::CodeUnavailable(message) => {
                self.code_error = Some(message);
                Vec::new()
            }
            E::Pairing(info) => {
                // Encode once here rather than per frame: the QR is static
                // until the ticket changes.
                self.qr = QrImage::encode(&info.ticket).ok();
                self.pairing = Some(info);
                Vec::new()
            }
            E::Offer(offer) => self.apply_offer(offer),
            E::Nearby(found) => {
                self.nearby = Some(found);
                self.send_to_cursor.clamp(self.send_to_rows().len());
                Vec::new()
            }
            E::Send(event) => self.apply_send(event),
            E::Fatal(message) => {
                self.fatal = Some(message);
                Vec::new()
            }
        }
    }

    fn apply_offer(&mut self, offer: ReceiverOfferEvent) -> Vec<AppRequest> {
        let phase = offer.phase;
        let mut requests = Vec::new();

        match phase {
            ReceiverOfferPhase::Connecting => {
                self.auto_accepted = false;
                self.offer_scroll = 0;
                self.text_scroll = 0;
                self.screen = Screen::Transfer;
            }
            ReceiverOfferPhase::OfferReady => {
                // Feature R6. An ephemeral sender (browser or CLI) has a
                // throwaway key, so "trusted" can never be a statement about
                // it — those always ask.
                let endpoint = offer.sender_endpoint_id.clone().unwrap_or_default();
                let trusted = !offer.sender_ephemeral && self.config.is_trusted(&endpoint);
                if trusted && !self.auto_accepted {
                    self.auto_accepted = true;
                    self.screen = Screen::Transfer;
                    let message = fill(self.s().toast_auto_accept, &offer.sender_name);
                    self.toast(message);
                    requests.push(AppRequest::Engine(EngineCommand::Respond(
                        OfferDecision::Accept(AcceptedDestinations::default()),
                    )));
                } else if !self.auto_accepted {
                    self.offer_scroll = 0;
                    self.screen = Screen::Offer;
                }
            }
            ReceiverOfferPhase::Receiving => {
                self.screen = Screen::Transfer;
            }
            ReceiverOfferPhase::Completed
            | ReceiverOfferPhase::Failed
            | ReceiverOfferPhase::Cancelled
            | ReceiverOfferPhase::Declined => {
                self.text_scroll = 0;
                self.screen = Screen::Result;
            }
        }

        self.offer = Some(offer);
        requests
    }

    fn apply_send(&mut self, event: SendEvent) -> Vec<AppRequest> {
        let mut requests = Vec::new();
        match event.phase {
            SendPhase::Completed
            | SendPhase::Declined
            | SendPhase::Failed
            | SendPhase::Cancelled => {
                self.screen = Screen::SendResult;
                // Only a completed transfer proves the ticket works, so only
                // that one is worth offering again later.
                if event.phase == SendPhase::Completed && !self.send_recorded {
                    self.send_recorded = true;
                    let reusable = !event.remote_ephemeral.unwrap_or(false);
                    if let (true, Some(id), Some(ticket)) = (
                        reusable,
                        event.remote_endpoint_id.as_deref(),
                        event.remote_ticket.as_deref(),
                    ) {
                        self.config
                            .remember_device(id, &event.destination_label, ticket);
                        requests.push(AppRequest::SaveConfig);
                    }
                }
            }
            _ => self.screen = Screen::SendProgress,
        }
        self.send_event = Some(event);
        requests
    }

    // ----------------------------------------------------------------- input

    pub fn handle_key(&mut self, event: KeyEvent) -> Vec<AppRequest> {
        self.last_key = Some(event);
        // Only act on presses; releases exist for the button tester.
        if !event.pressed {
            return Vec::new();
        }
        let Some(button) = event.button else {
            return Vec::new();
        };
        match self.screen {
            Screen::Home => self.on_home(button),
            Screen::Offer => self.on_offer(button),
            Screen::Transfer => self.on_transfer(button),
            Screen::Result => self.on_result(button),
            Screen::Settings => self.on_settings(button),
            Screen::SaveFolder => self.on_save_folder(button),
            Screen::Trusted => self.on_trusted(button),
            Screen::About => self.on_about(button),
            Screen::ButtonTest => self.on_button_test(button),
            Screen::SendPick => self.on_send_pick(button),
            Screen::SendTo => self.on_send_to(button),
            Screen::SendCode => self.on_send_code(button),
            Screen::SendProgress => self.on_send_progress(button),
            Screen::SendResult => self.on_send_result(button),
        }
    }

    fn on_home(&mut self, button: Button) -> Vec<AppRequest> {
        match button {
            Button::A => {
                self.toast(self.s().toast_new_code);
                vec![AppRequest::Engine(EngineCommand::RefreshCode)]
            }
            Button::Y | Button::Start => {
                self.settings_cursor = Cursor::default();
                self.screen = Screen::Settings;
                Vec::new()
            }
            Button::X => {
                self.open_send_picker();
                Vec::new()
            }
            Button::B => vec![AppRequest::Exit],
            _ => Vec::new(),
        }
    }

    // ------------------------------------------------------------------ send

    fn open_send_picker(&mut self) {
        // Start where files actually are. Falling back to the save folder's
        // parent keeps this usable off-device, where /mnt/SDCARD is absent.
        let start = [PathBuf::from("/mnt/SDCARD"), self.config.save_root.clone()]
            .into_iter()
            .find(|path| path.is_dir())
            .unwrap_or_else(|| PathBuf::from("/"));
        self.send_browser = Some(Browser::open(start, true));
        self.send_pick_cursor = Cursor::default();
        self.send_selection.clear();
        self.screen = Screen::SendPick;
    }

    /// Rows of the send picker: an "up" entry when there is a parent, then
    /// the directory's contents.
    fn send_pick_rows(&self) -> Vec<Row> {
        let Some(browser) = self.send_browser.as_ref() else {
            return Vec::new();
        };
        let strings = self.s();
        let mut rows = Vec::new();
        if browser.dir.parent().is_some() {
            rows.push(Row::new(".."));
        }
        for path in &browser.entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_owned();
            if path.is_dir() {
                rows.push(Row::new(name).with_value("›"));
            } else {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let row = Row::new(name);
                rows.push(if self.send_selection.contains(path) {
                    row.with_value(strings.send_picked)
                } else {
                    row.with_value(human_size(size))
                });
            }
        }
        rows
    }

    /// Maps a row index to the entry it shows, accounting for the leading
    /// "up" row. Shared with the renderer for the same reason the settings
    /// menu is: an index that means two different things is a bug waiting.
    fn send_pick_entry(&self, index: usize) -> Option<PathBuf> {
        let browser = self.send_browser.as_ref()?;
        let offset = usize::from(browser.dir.parent().is_some());
        if index < offset {
            return None;
        }
        browser.entries.get(index - offset).cloned()
    }

    fn on_send_pick(&mut self, button: Button) -> Vec<AppRequest> {
        let count = self.send_pick_rows().len();
        match button {
            Button::Up => self.send_pick_cursor.move_by(-1, count),
            Button::Down => self.send_pick_cursor.move_by(1, count),
            Button::A => {
                let index = self.send_pick_cursor.index;
                match self.send_pick_entry(index) {
                    None => {
                        if let Some(browser) = self.send_browser.as_mut() {
                            browser.up();
                        }
                        self.send_pick_cursor = Cursor::default();
                    }
                    Some(path) if path.is_dir() => {
                        if let Some(browser) = self.send_browser.as_mut() {
                            browser.enter(path);
                        }
                        self.send_pick_cursor = Cursor::default();
                    }
                    Some(path) => {
                        if let Some(at) =
                            self.send_selection.iter().position(|entry| *entry == path)
                        {
                            self.send_selection.remove(at);
                        } else {
                            self.send_selection.push(path);
                        }
                    }
                }
            }
            Button::Y => {
                // Whole folder in one press: the common case is "send this
                // directory", and ticking its files one by one is tedious.
                if let Some(path) = self.send_pick_entry(self.send_pick_cursor.index)
                    && path.is_dir()
                    && !self.send_selection.contains(&path)
                {
                    self.send_selection.push(path);
                }
            }
            Button::X => {
                if !self.send_selection.is_empty() {
                    return self.open_send_destinations();
                }
                self.toast(self.s().send_nothing_picked);
            }
            Button::B => {
                self.send_browser = None;
                self.screen = Screen::Home;
            }
            _ => {}
        }
        Vec::new()
    }

    fn open_send_destinations(&mut self) -> Vec<AppRequest> {
        self.nearby = None;
        self.send_to_cursor = Cursor::default();
        self.screen = Screen::SendTo;
        vec![AppRequest::Engine(EngineCommand::ScanNearby {
            timeout_secs: NEARBY_SCAN_SECS,
        })]
    }

    /// Destinations, in one list: devices found on the LAN, then devices sent
    /// to before, then the manual code entry.
    fn send_targets(&self) -> Vec<SendTarget> {
        let mut out = Vec::new();
        if let Some(Ok(found)) = self.nearby.as_ref() {
            for receiver in found {
                out.push(SendTarget::Ticket {
                    ticket: receiver.ticket.clone(),
                    label: receiver.label.clone(),
                });
            }
        }
        for device in &self.config.recent_devices {
            // A device discovered right now is already listed; showing it
            // twice would just split the user's attention.
            let already = out.iter().any(|target| match target {
                SendTarget::Ticket { ticket, .. } => *ticket == device.ticket,
                SendTarget::Code(_) => false,
            });
            if !already {
                out.push(SendTarget::Ticket {
                    ticket: device.ticket.clone(),
                    label: device.label.clone(),
                });
            }
        }
        out
    }

    fn send_to_rows(&self) -> Vec<Row> {
        let strings = self.s();
        let nearby_count = match self.nearby.as_ref() {
            Some(Ok(found)) => found.len(),
            _ => 0,
        };
        let mut rows: Vec<Row> = self
            .send_targets()
            .iter()
            .enumerate()
            .map(|(index, target)| {
                Row::new(target.label().to_owned()).with_subtitle(if index < nearby_count {
                    strings.send_via_nearby
                } else {
                    strings.send_via_recent
                })
            })
            .collect();

        match self.nearby.as_ref() {
            None => rows.push(Row::new(strings.send_scanning)),
            Some(Err(message)) => {
                rows.push(Row::new(strings.send_scan_failed).with_subtitle(message.clone()))
            }
            Some(Ok(_)) => {}
        }
        rows.push(Row::new(strings.send_enter_code).with_value("›"));
        rows
    }

    fn on_send_to(&mut self, button: Button) -> Vec<AppRequest> {
        let rows = self.send_to_rows().len();
        match button {
            Button::Up => self.send_to_cursor.move_by(-1, rows),
            Button::Down => self.send_to_cursor.move_by(1, rows),
            Button::Y => return self.open_send_destinations(),
            Button::B => {
                self.screen = Screen::SendPick;
            }
            Button::A => {
                let targets = self.send_targets();
                let index = self.send_to_cursor.index;
                if let Some(target) = targets.get(index).cloned() {
                    return self.start_send(target);
                }
                // Anything past the targets is either the scan status line,
                // which does nothing, or the code entry, which is last.
                if index + 1 == rows {
                    self.code_input.clear();
                    self.code_key = 0;
                    self.screen = Screen::SendCode;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn start_send(&mut self, destination: SendTarget) -> Vec<AppRequest> {
        self.send_event = None;
        self.send_recorded = false;
        self.screen = Screen::SendProgress;
        vec![AppRequest::Engine(EngineCommand::StartSend {
            paths: self.send_selection.clone(),
            destination,
        })]
    }

    fn on_send_code(&mut self, button: Button) -> Vec<AppRequest> {
        let keys = CODE_KEYS.len();
        match button {
            Button::Left => self.code_key = self.code_key.saturating_sub(1),
            Button::Right => self.code_key = (self.code_key + 1).min(keys - 1),
            Button::Up => self.code_key = self.code_key.saturating_sub(CODE_COLUMNS),
            Button::Down => self.code_key = (self.code_key + CODE_COLUMNS).min(keys - 1),
            Button::A => {
                if self.code_input.chars().count() < CODE_LENGTH {
                    self.code_input.push(CODE_KEYS[self.code_key]);
                }
            }
            Button::X => {
                self.code_input.pop();
            }
            Button::Start => {
                if self.code_input.chars().count() == CODE_LENGTH {
                    let code = self.code_input.clone();
                    return self.start_send(SendTarget::Code(code));
                }
            }
            Button::B => self.screen = Screen::SendTo,
            _ => {}
        }
        Vec::new()
    }

    fn on_send_progress(&mut self, button: Button) -> Vec<AppRequest> {
        if button == Button::B {
            self.toast(self.s().toast_cancelling);
            return vec![AppRequest::Engine(EngineCommand::CancelSend)];
        }
        Vec::new()
    }

    fn on_send_result(&mut self, button: Button) -> Vec<AppRequest> {
        match button {
            Button::A | Button::B => {
                self.send_event = None;
                self.send_selection.clear();
                self.send_browser = None;
                self.screen = Screen::Home;
            }
            Button::X => {
                // Same selection, different destination.
                return self.open_send_destinations();
            }
            _ => {}
        }
        Vec::new()
    }

    fn on_offer(&mut self, button: Button) -> Vec<AppRequest> {
        match button {
            Button::A => {
                self.screen = Screen::Transfer;
                vec![AppRequest::Engine(EngineCommand::Respond(
                    OfferDecision::Accept(AcceptedDestinations::default()),
                ))]
            }
            Button::B => vec![AppRequest::Engine(EngineCommand::Respond(
                OfferDecision::Decline,
            ))],
            Button::X => {
                // Trust, then accept — the pairing of the two is the whole
                // point of the shortcut.
                let mut requests = Vec::new();
                if let Some(offer) = self.offer.clone() {
                    let endpoint = offer.sender_endpoint_id.clone().unwrap_or_default();
                    if offer.sender_ephemeral || endpoint.is_empty() {
                        self.toast(self.s().toast_no_identity);
                    } else {
                        self.config.trust(&endpoint, &offer.sender_name);
                        let message = fill(self.s().toast_trusted, &offer.sender_name);
                        self.toast(message);
                        requests.push(AppRequest::SaveConfig);
                    }
                }
                self.screen = Screen::Transfer;
                requests.push(AppRequest::Engine(EngineCommand::Respond(
                    OfferDecision::Accept(AcceptedDestinations::default()),
                )));
                requests
            }
            Button::Up => {
                self.offer_scroll = self.offer_scroll.saturating_sub(1);
                Vec::new()
            }
            Button::Down => {
                let count = self.offer.as_ref().map(|o| o.files.len()).unwrap_or(0);
                if self.offer_scroll + 1 < count {
                    self.offer_scroll += 1;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn on_transfer(&mut self, button: Button) -> Vec<AppRequest> {
        match button {
            Button::B => {
                self.toast(self.s().toast_cancelling);
                vec![AppRequest::Engine(EngineCommand::Cancel)]
            }
            _ => Vec::new(),
        }
    }

    fn on_result(&mut self, button: Button) -> Vec<AppRequest> {
        match button {
            Button::A | Button::B => {
                self.screen = Screen::Home;
                self.offer = None;
                self.auto_accepted = false;
                Vec::new()
            }
            Button::X => {
                let Some(offer) = self.offer.clone() else {
                    return Vec::new();
                };
                let endpoint = offer.sender_endpoint_id.clone().unwrap_or_default();
                if offer.sender_ephemeral || endpoint.is_empty() {
                    self.toast(self.s().toast_no_identity);
                    return Vec::new();
                }
                let message = if self.config.is_trusted(&endpoint) {
                    self.config.untrust(&endpoint);
                    fill(self.s().toast_untrusted, &offer.sender_name)
                } else {
                    self.config.trust(&endpoint, &offer.sender_name);
                    fill(self.s().toast_trusted, &offer.sender_name)
                };
                self.toast(message);
                vec![AppRequest::SaveConfig]
            }
            Button::Up => {
                self.text_scroll = self.text_scroll.saturating_sub(1);
                Vec::new()
            }
            Button::Down => {
                self.text_scroll += 1;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Settings rows, in the order they are drawn.
    fn settings_rows(&self) -> Vec<Row> {
        SettingsRow::ORDER
            .iter()
            .map(|row| row.render(self))
            .collect()
    }

    fn on_settings(&mut self, button: Button) -> Vec<AppRequest> {
        let count = self.settings_rows().len();
        match button {
            Button::Up => {
                self.settings_cursor.move_by(-1, count);
                Vec::new()
            }
            Button::Down => {
                self.settings_cursor.move_by(1, count);
                Vec::new()
            }
            Button::B => {
                self.screen = Screen::Home;
                Vec::new()
            }
            Button::A => match SettingsRow::at(self.settings_cursor.index) {
                Some(SettingsRow::SaveFolder) => {
                    self.folder_cursor = Cursor::default();
                    self.browser = None;
                    self.screen = Screen::SaveFolder;
                    Vec::new()
                }
                Some(SettingsRow::Conflict) => {
                    self.config.conflict = self.config.conflict.toggled();
                    let message = fill(
                        self.s().toast_clash_policy,
                        self.config.conflict.label(self.config.lang),
                    );
                    self.toast(message);
                    // The policy is baked into the running receiver.
                    vec![AppRequest::SaveConfig, AppRequest::RestartEngine]
                }
                Some(SettingsRow::Language) => {
                    self.config.lang = self.config.lang.toggled();
                    // Language is drawn, not served: the receiver keeps
                    // running and only the next frame changes.
                    let message = fill(self.s().toast_language, self.config.lang.label());
                    self.toast(message);
                    vec![AppRequest::SaveConfig]
                }
                Some(SettingsRow::Trusted) => {
                    self.trusted_cursor = Cursor::default();
                    self.screen = Screen::Trusted;
                    Vec::new()
                }
                Some(SettingsRow::ButtonTest) => {
                    self.screen = Screen::ButtonTest;
                    Vec::new()
                }
                Some(SettingsRow::About) => {
                    self.screen = Screen::About;
                    Vec::new()
                }
                Some(SettingsRow::DeviceName) | None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Folder picker rows: the shortcuts, then the current directory's
    /// children once the user starts browsing.
    /// The folders offered on the picker's first page, in display order:
    /// previously chosen ones first, then the built-in suggestions that are
    /// not already among them.
    ///
    /// Shared by `folder_rows` and `choose_folder` so the row a user sees and
    /// the folder that gets selected cannot drift apart.
    fn folder_candidates(&self) -> Vec<(PathBuf, bool)> {
        let recent = self.config.recent_save_roots_present();
        let mut out: Vec<(PathBuf, bool)> =
            recent.iter().map(|path| (path.clone(), true)).collect();
        for path in Config::save_root_suggestions() {
            if !recent.contains(&path) {
                out.push((path, false));
            }
        }
        out
    }

    fn folder_rows(&self) -> Vec<Row> {
        match &self.browser {
            None => {
                let strings = self.s();
                let candidates = self.folder_candidates();
                // Group labels only earn their space when there are two groups
                // to tell apart; on a fresh install everything is a suggestion
                // and saying so on every screen is noise.
                let mixed = candidates.iter().any(|(_, recent)| *recent)
                    && candidates.iter().any(|(_, recent)| !*recent);
                let mut previous_recent: Option<bool> = None;
                let mut rows: Vec<Row> = candidates
                    .iter()
                    .map(|(path, is_recent)| {
                        let mut row = Row::new(path.display().to_string());
                        // Label only the first row of each group: a heading on
                        // every row would drown the paths it is grouping.
                        if mixed && previous_recent != Some(*is_recent) {
                            row = row.with_subtitle(if *is_recent {
                                strings.folder_recent
                            } else {
                                strings.folder_suggested
                            });
                        }
                        previous_recent = Some(*is_recent);
                        if *path == self.config.save_root {
                            row = row.with_value(strings.folder_in_use);
                        }
                        row
                    })
                    .collect();
                rows.push(Row::new(strings.folder_browse).with_value("›"));
                rows
            }
            Some(browser) => {
                let mut rows = vec![
                    Row::new(self.s().folder_use_this)
                        .with_subtitle(browser.dir.display().to_string()),
                ];
                if browser.dir.parent().is_some() {
                    rows.push(Row::new(".."));
                }
                rows.extend(browser.entries.iter().map(|path| {
                    Row::new(
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?")
                            .to_owned(),
                    )
                    .with_value("›")
                }));
                rows
            }
        }
    }

    fn on_save_folder(&mut self, button: Button) -> Vec<AppRequest> {
        let count = self.folder_rows().len();
        match button {
            Button::Up => {
                self.folder_cursor.move_by(-1, count);
                Vec::new()
            }
            Button::Down => {
                self.folder_cursor.move_by(1, count);
                Vec::new()
            }
            Button::B => {
                // Back out of the browser one level at a time, then to settings.
                match self.browser.as_mut() {
                    Some(_) => {
                        self.browser = None;
                        self.folder_cursor = Cursor::default();
                    }
                    None => self.screen = Screen::Settings,
                }
                Vec::new()
            }
            Button::A => self.choose_folder(),
            _ => Vec::new(),
        }
    }

    fn choose_folder(&mut self) -> Vec<AppRequest> {
        let index = self.folder_cursor.index;
        match self.browser.as_mut() {
            None => {
                let candidates = self.folder_candidates();
                if let Some((path, _)) = candidates.get(index).cloned() {
                    return self.set_save_root(path);
                }
                // "Duyệt thư mục…"
                let start = PathBuf::from("/mnt/SDCARD");
                let start = if start.is_dir() {
                    start
                } else {
                    // Off-device (tests, a desktop run) there is no SD card.
                    self.config
                        .save_root
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from("/"))
                };
                // Directories only: this picker chooses a destination folder.
                self.browser = Some(Browser::open(start, false));
                self.folder_cursor = Cursor::default();
                Vec::new()
            }
            Some(browser) => {
                if index == 0 {
                    let chosen = browser.dir.clone();
                    return self.set_save_root(chosen);
                }
                let has_parent = browser.dir.parent().is_some();
                if has_parent && index == 1 {
                    browser.up();
                    self.folder_cursor = Cursor::default();
                    return Vec::new();
                }
                let entry_index = if has_parent { index - 2 } else { index - 1 };
                if let Some(path) = browser.entries.get(entry_index).cloned() {
                    browser.enter(path);
                    self.folder_cursor = Cursor::default();
                }
                Vec::new()
            }
        }
    }

    fn set_save_root(&mut self, path: PathBuf) -> Vec<AppRequest> {
        // Remember it either way: re-picking the folder already in use is
        // still a signal that it is the one the user reaches for.
        self.config.remember_save_root(&path);
        if path == self.config.save_root {
            self.browser = None;
            self.screen = Screen::Settings;
            return vec![AppRequest::SaveConfig];
        }
        self.config.save_root = path;
        self.toast(self.s().folder_changed);
        self.browser = None;
        self.screen = Screen::Settings;
        vec![AppRequest::SaveConfig, AppRequest::RestartEngine]
    }

    fn on_trusted(&mut self, button: Button) -> Vec<AppRequest> {
        let count = self.config.trusted.len();
        match button {
            Button::Up => {
                self.trusted_cursor.move_by(-1, count);
                Vec::new()
            }
            Button::Down => {
                self.trusted_cursor.move_by(1, count);
                Vec::new()
            }
            Button::B => {
                self.screen = Screen::Settings;
                Vec::new()
            }
            Button::X => {
                if let Some(device) = self.config.trusted.get(self.trusted_cursor.index).cloned() {
                    self.config.untrust(&device.endpoint_id);
                    self.trusted_cursor.clamp(self.config.trusted.len());
                    let message = fill(self.s().toast_removed, &device.name);
                    self.toast(message);
                    return vec![AppRequest::SaveConfig];
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn on_about(&mut self, button: Button) -> Vec<AppRequest> {
        if matches!(button, Button::A | Button::B) {
            self.screen = Screen::Settings;
        }
        Vec::new()
    }

    fn on_button_test(&mut self, button: Button) -> Vec<AppRequest> {
        // Start is the only way out, so every other button stays testable.
        if button == Button::Start {
            self.screen = Screen::Settings;
        }
        Vec::new()
    }

    // ---------------------------------------------------------------- render

    pub fn render(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        canvas.clear(theme::BG);

        if let Some(message) = self.fatal.clone() {
            self.render_fatal(canvas, fonts, &message);
            return;
        }

        match self.screen {
            Screen::Home => self.render_home(canvas, fonts),
            Screen::Offer => self.render_offer(canvas, fonts),
            Screen::Transfer => self.render_transfer(canvas, fonts),
            Screen::Result => self.render_result(canvas, fonts),
            Screen::Settings => self.render_settings(canvas, fonts),
            Screen::SaveFolder => self.render_save_folder(canvas, fonts),
            Screen::Trusted => self.render_trusted(canvas, fonts),
            Screen::About => self.render_about(canvas, fonts),
            Screen::ButtonTest => self.render_button_test(canvas, fonts),
            Screen::SendPick => self.render_send_pick(canvas, fonts),
            Screen::SendTo => self.render_send_to(canvas, fonts),
            Screen::SendCode => self.render_send_code(canvas, fonts),
            Screen::SendProgress => self.render_send_progress(canvas, fonts),
            Screen::SendResult => self.render_send_result(canvas, fonts),
        }

        self.render_toast(canvas, fonts);
    }

    fn render_fatal(&self, canvas: &mut Canvas, fonts: &mut Fonts, message: &str) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, "Wisp", None);

        // Size the panel to the message. A fixed height used to clip the last
        // line of a multi-paragraph explanation — which is exactly the text a
        // stuck user needs to read.
        let title_height = fonts.line_height(text::HEADING);
        let body_width = content.w - metrics::GAP * 2;
        let body_height = fonts.wrap(message, text::BODY, body_width).len() as i32
            * fonts.line_height(text::BODY);
        let panel_height = (metrics::GAP * 2 + title_height + 8 + body_height).min(content.h);

        let panel_rect = Rect::new(content.x, content.y, content.w, panel_height);
        ui::panel(canvas, panel_rect);
        let inner = panel_rect.inset(metrics::GAP);
        fonts.draw(
            canvas,
            inner.x,
            inner.y,
            strings.fatal_title,
            text::HEADING,
            theme::DANGER,
            Weight::Bold,
        );
        ui::paragraph(
            canvas,
            fonts,
            Rect::new(
                inner.x,
                inner.y + title_height + 8,
                inner.w,
                inner.h - title_height - 8,
            ),
            message,
            text::BODY,
            theme::TEXT_MUTED,
            0,
        );
        ui::footer(canvas, fonts, &[("B", strings.hint_exit)]);
    }

    fn render_home(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let status = if self.ready {
            strings.status_ready
        } else {
            strings.status_starting
        };
        let content = ui::header(canvas, fonts, "Wisp", Some(status));

        // Left column carries the code and the destination; the right holds
        // the QR. Both run the full height of the content area so the two
        // columns line up top and bottom.
        let qr_width = content.h.min(440);
        let right = Rect::new(content.right() - qr_width, content.y, qr_width, content.h);
        let left = Rect::new(
            content.x,
            content.y,
            content.w - qr_width - metrics::GAP,
            content.h,
        );

        // --- pairing code
        let code_panel = Rect::new(left.x, left.y, left.w, 180);
        ui::panel(canvas, code_panel);
        let inner = code_panel.inset(metrics::GAP);
        fonts.draw(
            canvas,
            inner.x,
            inner.y,
            strings.pairing_code,
            text::SMALL,
            theme::TEXT_MUTED,
            Weight::Regular,
        );

        let (code_text, code_color, note) = match (&self.code, &self.code_error) {
            (PairingCodeState::Active(registration), _) => (
                spaced_code(&registration.code),
                theme::TEXT,
                format_expiry(&registration.expires_at, OffsetDateTime::now_utc(), strings),
            ),
            (PairingCodeState::Stale(registration), _) => (
                spaced_code(&registration.code),
                theme::TEXT_FAINT,
                Some(strings.code_stale_hint.to_owned()),
            ),
            (PairingCodeState::Unavailable, Some(_)) => (
                "— — —".to_owned(),
                theme::TEXT_FAINT,
                Some(strings.code_offline_hint.to_owned()),
            ),
            (PairingCodeState::Unavailable, None) => ("— — —".to_owned(), theme::TEXT_FAINT, None),
        };

        let code_top = inner.y + 26;
        fonts.draw(
            canvas,
            inner.x,
            code_top,
            &code_text,
            text::CODE,
            code_color,
            Weight::Bold,
        );
        if let Some(note) = note {
            // Sit below the code's own line box rather than at a fixed offset:
            // a hardcoded 72 px left the note riding on the descenders of the
            // 64 px code on the device.
            let note_top = code_top + fonts.line_height(text::CODE) + 4;
            fonts.draw_in(
                canvas,
                Rect::new(inner.x, note_top, inner.w, fonts.line_height(text::SMALL)),
                &note,
                text::SMALL,
                theme::TEXT_FAINT,
                Weight::Regular,
                Align::Left,
            );
        }

        // --- destination + network
        let info_top = code_panel.bottom() + metrics::GAP;
        let info_panel = Rect::new(left.x, info_top, left.w, left.bottom() - info_top);
        ui::panel(canvas, info_panel);
        let inner = info_panel.inset(metrics::GAP);
        let mut y = inner.y;

        for (label, value) in [
            (strings.save_to, self.config.save_root.display().to_string()),
            (strings.device_name, self.config.device_name.clone()),
            (
                strings.identity,
                // Shortened: the full base32 key is 52 characters and would
                // wrap. The head and tail are enough to compare against what
                // the sender shows.
                self.endpoint_id
                    .as_deref()
                    .map(short_key)
                    .unwrap_or_else(|| "—".to_owned()),
            ),
        ] {
            fonts.draw(
                canvas,
                inner.x,
                y,
                label,
                text::SMALL,
                theme::TEXT_MUTED,
                Weight::Regular,
            );
            y += fonts.line_height(text::SMALL);
            fonts.draw_in(
                canvas,
                Rect::new(inner.x, y, inner.w, 30),
                &value,
                text::BODY,
                theme::TEXT,
                Weight::Regular,
                Align::Left,
            );
            y += fonts.line_height(text::BODY) + metrics::GAP;
        }

        let ips = self
            .pairing
            .as_ref()
            .map(|info| info.lan_ips.clone())
            .unwrap_or_default();
        fonts.draw(
            canvas,
            inner.x,
            y,
            strings.lan_address,
            text::SMALL,
            theme::TEXT_MUTED,
            Weight::Regular,
        );
        y += fonts.line_height(text::SMALL);
        let ip_text = if ips.is_empty() {
            strings.no_wifi.to_owned()
        } else {
            ips.join("  ")
        };
        fonts.draw_in(
            canvas,
            Rect::new(inner.x, y, inner.w, 30),
            &ip_text,
            text::SMALL,
            if ips.is_empty() {
                theme::WARNING
            } else {
                theme::TEXT_MUTED
            },
            Weight::Regular,
            Align::Left,
        );

        // --- QR
        ui::panel(canvas, right);
        match &self.qr {
            Some(qr) => {
                // Reserve the caption line first, then give the QR the rest,
                // so the caption sits inside the panel instead of straddling
                // its bottom border.
                let caption_height = fonts.line_height(text::SMALL) + metrics::GAP;
                let area = Rect::new(
                    right.x + metrics::GAP,
                    right.y + metrics::GAP,
                    right.w - metrics::GAP * 2,
                    right.h - metrics::GAP * 2 - caption_height,
                );
                qr.draw(canvas, area, theme::QR_DARK, theme::QR_LIGHT);
                fonts.draw_in(
                    canvas,
                    Rect::new(
                        right.x,
                        right.bottom() - caption_height,
                        right.w,
                        caption_height,
                    ),
                    strings.scan_to_pair,
                    text::SMALL,
                    theme::TEXT_MUTED,
                    Weight::Regular,
                    Align::Center,
                );
            }
            None => {
                fonts.draw_in(
                    canvas,
                    Rect::new(right.x, right.y + right.h / 2 - 14, right.w, 30),
                    strings.building_qr,
                    text::BODY,
                    theme::TEXT_FAINT,
                    Weight::Regular,
                    Align::Center,
                );
            }
        }

        ui::footer(
            canvas,
            fonts,
            &[
                ("X", strings.hint_send),
                ("A", strings.hint_new_code),
                ("Y", strings.hint_settings),
                ("B", strings.hint_exit),
            ],
        );
    }

    fn render_offer(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.offer_title, None);
        let Some(offer) = self.offer.clone() else {
            return;
        };

        let head = Rect::new(content.x, content.y, content.w, 110);
        ui::panel(canvas, head);
        let inner = head.inset(metrics::GAP);
        fonts.draw_in(
            canvas,
            Rect::new(inner.x, inner.y, inner.w, 36),
            &offer.sender_name,
            text::HEADING,
            theme::TEXT,
            Weight::Bold,
            Align::Left,
        );
        let summary = fill2(
            plural(
                offer.item_count,
                strings.offer_summary_one,
                strings.offer_summary,
            ),
            &offer.item_count.to_string(),
            &human_size(offer.total_size_bytes),
        );
        fonts.draw(
            canvas,
            inner.x,
            inner.y + fonts.line_height(text::HEADING) + 4,
            &summary,
            text::BODY,
            theme::ACCENT,
            Weight::Regular,
        );

        if offer.sender_web {
            ui::pill(
                canvas,
                fonts,
                inner.right() - 110,
                inner.y + 4,
                strings.pill_browser,
                theme::ON_ACCENT,
                theme::ACCENT,
            );
        }

        let list_top = head.bottom() + metrics::GAP;
        let list_area = Rect::new(content.x, list_top, content.w, content.bottom() - list_top);

        if let Some(body) = offer.inline_text.as_deref() {
            ui::panel(canvas, list_area);
            let inner = list_area.inset(metrics::GAP);
            fonts.draw(
                canvas,
                inner.x,
                inner.y,
                strings.text_content,
                text::SMALL,
                theme::TEXT_MUTED,
                Weight::Regular,
            );
            ui::paragraph(
                canvas,
                fonts,
                Rect::new(
                    inner.x,
                    inner.y + fonts.line_height(text::SMALL) + 6,
                    inner.w,
                    inner.h - fonts.line_height(text::SMALL) - 6,
                ),
                body,
                text::BODY,
                theme::TEXT,
                self.offer_scroll,
            );
        } else {
            let rows: Vec<Row> = offer
                .files
                .iter()
                .map(|file| Row::new(file.path.clone()).with_value(human_size(file.size)))
                .collect();
            // No selection: the manifest is there to be read, not picked from.
            self.offer_scroll = ui::list(
                canvas,
                fonts,
                list_area,
                &rows,
                None,
                self.offer_scroll,
                strings.list_empty,
            );
        }

        ui::footer(
            canvas,
            fonts,
            &[
                ("A", strings.hint_accept),
                ("B", strings.hint_decline),
                ("X", strings.hint_always_trust),
            ],
        );
    }

    fn render_transfer(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.transfer_title, None);
        let Some(offer) = self.offer.clone() else {
            return;
        };

        let panel_rect = Rect::new(content.x, content.y, content.w, 260);
        ui::panel(canvas, panel_rect);
        let inner = panel_rect.inset(metrics::GAP + 6);

        fonts.draw_in(
            canvas,
            Rect::new(inner.x, inner.y, inner.w, 36),
            &offer.sender_name,
            text::HEADING,
            theme::TEXT,
            Weight::Bold,
            Align::Left,
        );

        let status = if offer.status_message.is_empty() {
            match offer.phase {
                ReceiverOfferPhase::Connecting => strings.connecting.to_owned(),
                _ => strings.transferring.to_owned(),
            }
        } else {
            offer.status_message.clone()
        };
        fonts.draw_in(
            canvas,
            Rect::new(
                inner.x,
                inner.y + fonts.line_height(text::HEADING) + 6,
                inner.w,
                30,
            ),
            &status,
            text::BODY,
            theme::TEXT_MUTED,
            Weight::Regular,
            Align::Left,
        );

        let fraction = if offer.total_size_bytes > 0 {
            offer.bytes_received as f32 / offer.total_size_bytes as f32
        } else {
            0.0
        };
        let bar = Rect::new(inner.x, inner.y + 120, inner.w, 16);
        ui::progress_bar(canvas, bar, fraction, theme::ACCENT_STRONG);

        let transferred = format!(
            "{} / {}",
            human_size(offer.bytes_received),
            human_size(offer.total_size_bytes)
        );
        fonts.draw(
            canvas,
            inner.x,
            bar.bottom() + 12,
            &transferred,
            text::BODY,
            theme::TEXT,
            Weight::Regular,
        );

        let mut right_parts: Vec<String> = Vec::new();
        if let Some(snapshot) = offer.snapshot.as_ref() {
            right_parts.push(fill2(
                strings.files_progress,
                &snapshot.completed_files.to_string(),
                &snapshot.total_files.to_string(),
            ));
            if let Some(rate) = snapshot.bytes_per_sec {
                right_parts.push(format!("{}/s", human_size(rate)));
            }
            if let Some(eta) = snapshot.eta_seconds {
                right_parts.push(fill(strings.eta_left, &format_duration(eta)));
            }
        }
        if !right_parts.is_empty() {
            let line = right_parts.join(" · ");
            let width = fonts.measure(&line, text::BODY);
            fonts.draw(
                canvas,
                inner.right() - width,
                bar.bottom() + 12,
                &line,
                text::BODY,
                theme::ACCENT,
                Weight::Regular,
            );
        }

        let percent = format!("{}%", (fraction * 100.0).round() as i32);
        fonts.draw_in(
            canvas,
            Rect::new(inner.x, bar.y - 48, inner.w, 44),
            &percent,
            text::TITLE,
            theme::TEXT,
            Weight::Bold,
            Align::Right,
        );

        // The manifest fills the rest of the screen, so the user can see what
        // is still coming rather than watching a bar over an empty page.
        if !offer.files.is_empty() {
            let list_top = panel_rect.bottom() + metrics::GAP;
            let rows: Vec<Row> = offer
                .files
                .iter()
                .map(|file| Row::new(file.path.clone()).with_value(human_size(file.size)))
                .collect();
            ui::list(
                canvas,
                fonts,
                Rect::new(content.x, list_top, content.w, content.bottom() - list_top),
                &rows,
                None,
                0,
                strings.list_empty,
            );
        }

        ui::footer(canvas, fonts, &[("B", strings.hint_cancel)]);
    }

    fn render_result(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let Some(offer) = self.offer.clone() else {
            self.screen = Screen::Home;
            return;
        };

        let (title, color, headline) = match offer.phase {
            ReceiverOfferPhase::Completed => (
                strings.result_completed,
                theme::SUCCESS,
                fill2(
                    plural(
                        offer.item_count,
                        strings.received_summary_one,
                        strings.received_summary,
                    ),
                    &offer.item_count.to_string(),
                    &human_size(offer.total_size_bytes),
                ),
            ),
            ReceiverOfferPhase::Declined => (
                strings.result_declined,
                theme::TEXT_MUTED,
                strings.declined_body.to_owned(),
            ),
            ReceiverOfferPhase::Cancelled => (
                strings.result_cancelled,
                theme::WARNING,
                strings.cancelled_body.to_owned(),
            ),
            _ => (
                strings.result_failed,
                theme::DANGER,
                offer
                    .error
                    .as_ref()
                    .map(|error| error.title().to_owned())
                    .unwrap_or_else(|| strings.failed_body.to_owned()),
            ),
        };

        let content = ui::header(canvas, fonts, title, None);
        let head = Rect::new(content.x, content.y, content.w, 120);
        ui::panel(canvas, head);
        let inner = head.inset(metrics::GAP);
        fonts.draw_in(
            canvas,
            Rect::new(inner.x, inner.y, inner.w, 36),
            &headline,
            text::HEADING,
            color,
            Weight::Bold,
            Align::Left,
        );
        let detail = match offer.phase {
            ReceiverOfferPhase::Completed => self.config.save_root.display().to_string(),
            _ => offer
                .error
                .as_ref()
                .map(|error| error.message().to_owned())
                .unwrap_or_else(|| offer.status_message.clone()),
        };
        fonts.draw_in(
            canvas,
            Rect::new(
                inner.x,
                inner.y + fonts.line_height(text::HEADING) + 6,
                inner.w,
                30,
            ),
            &detail,
            text::BODY,
            theme::TEXT_MUTED,
            Weight::Regular,
            Align::Left,
        );

        // Feature R4: a text/link payload is the content, so it gets the rest
        // of the screen and its own scrolling.
        let body_top = head.bottom() + metrics::GAP;
        let body = Rect::new(content.x, body_top, content.w, content.bottom() - body_top);
        if let Some(inline) = offer.inline_text.as_deref() {
            ui::panel(canvas, body);
            let inner = body.inset(metrics::GAP);
            let total = ui::paragraph(
                canvas,
                fonts,
                inner,
                inline,
                text::BODY,
                theme::TEXT,
                self.text_scroll,
            );
            let visible = (inner.h / fonts.line_height(text::BODY)).max(1) as usize;
            // Clamp here rather than in the handler: the line count depends on
            // the rendered width, which only this function knows.
            self.text_scroll = self.text_scroll.min(total.saturating_sub(visible));
        } else if !offer.files.is_empty() {
            let rows: Vec<Row> = offer
                .files
                .iter()
                .map(|file| Row::new(file.path.clone()).with_value(human_size(file.size)))
                .collect();
            self.text_scroll = ui::list(
                canvas,
                fonts,
                body,
                &rows,
                None,
                self.text_scroll,
                strings.list_empty,
            );
        }

        let endpoint = offer.sender_endpoint_id.clone().unwrap_or_default();
        let can_trust = !offer.sender_ephemeral && !endpoint.is_empty();
        let trust_hint = if self.config.is_trusted(&endpoint) {
            strings.hint_untrust
        } else {
            strings.hint_trust
        };
        let mut hints: Vec<(&str, &str)> = vec![("A", strings.hint_done)];
        if can_trust {
            hints.push(("X", trust_hint));
        }
        ui::footer(canvas, fonts, &hints);
    }

    fn render_settings(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.settings_title, None);
        let rows = self.settings_rows();
        self.settings_cursor.clamp(rows.len());
        self.settings_cursor.scroll = ui::list(
            canvas,
            fonts,
            content,
            &rows,
            Some(self.settings_cursor.index),
            self.settings_cursor.scroll,
            strings.list_empty,
        );
        ui::footer(
            canvas,
            fonts,
            &[("A", strings.hint_select), ("B", strings.hint_back)],
        );
    }

    fn render_save_folder(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let title = match &self.browser {
            Some(browser) => browser.dir.display().to_string(),
            None => strings.folder_title.to_owned(),
        };
        let content = ui::header(canvas, fonts, &title, None);
        let rows = self.folder_rows();
        self.folder_cursor.clamp(rows.len());
        self.folder_cursor.scroll = ui::list(
            canvas,
            fonts,
            content,
            &rows,
            Some(self.folder_cursor.index),
            self.folder_cursor.scroll,
            strings.list_empty,
        );
        ui::footer(
            canvas,
            fonts,
            &[("A", strings.hint_select), ("B", strings.hint_back)],
        );
    }

    fn render_trusted(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.trusted_title, None);
        let rows: Vec<Row> = self
            .config
            .trusted
            .iter()
            .map(|device| {
                Row::new(device.name.clone()).with_subtitle(short_key(&device.endpoint_id))
            })
            .collect();
        self.trusted_cursor.clamp(rows.len());
        self.trusted_cursor.scroll = ui::list(
            canvas,
            fonts,
            content,
            &rows,
            Some(self.trusted_cursor.index),
            self.trusted_cursor.scroll,
            strings.list_empty,
        );
        ui::footer(
            canvas,
            fonts,
            &[("X", strings.hint_remove), ("B", strings.hint_back)],
        );
    }

    fn render_about(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.about_title, None);
        ui::panel(canvas, content);
        let inner = content.inset(metrics::GAP * 2);
        let mut y = inner.y;

        // The identity is the one long value here, so it gets the full width
        // and no truncation — the point of showing it is to compare it, in
        // full, against what the sender displays.
        let entries: [(&str, String, Option<&str>); 4] = [
            (
                strings.about_version,
                env!("CARGO_PKG_VERSION").to_owned(),
                None,
            ),
            (
                strings.about_identity,
                self.endpoint_id.clone().unwrap_or_else(|| "—".to_owned()),
                Some(strings.about_identity_hint),
            ),
            (
                strings.about_save_folder,
                self.config.save_root.display().to_string(),
                None,
            ),
            (strings.device_name, self.config.device_name.clone(), None),
        ];

        for (label, value, hint) in entries {
            fonts.draw(
                canvas,
                inner.x,
                y,
                label,
                text::SMALL,
                theme::TEXT_MUTED,
                Weight::Regular,
            );
            y += fonts.line_height(text::SMALL);
            let lines = fonts.wrap(&value, text::BODY, inner.w);
            for line in &lines {
                fonts.draw(
                    canvas,
                    inner.x,
                    y,
                    line,
                    text::BODY,
                    theme::TEXT,
                    Weight::Regular,
                );
                y += fonts.line_height(text::BODY);
            }
            if let Some(hint) = hint {
                fonts.draw(
                    canvas,
                    inner.x,
                    y,
                    hint,
                    text::SMALL,
                    theme::TEXT_FAINT,
                    Weight::Regular,
                );
                y += fonts.line_height(text::SMALL);
            }
            y += metrics::GAP;
        }

        fonts.draw_in(
            canvas,
            Rect::new(inner.x, content.bottom() - 48, inner.w, 30),
            strings.about_project,
            text::SMALL,
            theme::ACCENT,
            Weight::Regular,
            Align::Left,
        );

        ui::footer(canvas, fonts, &[("B", strings.hint_back)]);
    }

    fn render_button_test(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.button_test_title, None);
        ui::panel(canvas, content);
        let inner = content.inset(metrics::GAP * 2);

        ui::paragraph(
            canvas,
            fonts,
            Rect::new(inner.x, inner.y, inner.w, 90),
            strings.button_test_hint,
            text::BODY,
            theme::TEXT_MUTED,
            0,
        );

        let y = inner.y + 110;
        match self.last_key {
            Some(event) => {
                let name = event
                    .button
                    .map(|button| button.label().to_owned())
                    .unwrap_or_else(|| strings.button_unmapped.to_owned());
                fonts.draw_in(
                    canvas,
                    Rect::new(inner.x, y, inner.w, 70),
                    &name,
                    text::TITLE,
                    if event.button.is_some() {
                        theme::ACCENT
                    } else {
                        theme::WARNING
                    },
                    Weight::Bold,
                    Align::Center,
                );
                let detail = format!(
                    "type {} · code {} · value {}",
                    event.raw_type, event.raw_code, event.raw_value
                );
                fonts.draw_in(
                    canvas,
                    Rect::new(inner.x, y + 70, inner.w, 30),
                    &detail,
                    text::BODY,
                    theme::TEXT_MUTED,
                    Weight::Regular,
                    Align::Center,
                );
            }
            None => {
                fonts.draw_in(
                    canvas,
                    Rect::new(inner.x, y, inner.w, 40),
                    strings.button_none_yet,
                    text::BODY,
                    theme::TEXT_FAINT,
                    Weight::Regular,
                    Align::Center,
                );
            }
        }

        ui::footer(canvas, fonts, &[("Start", strings.hint_back)]);
    }

    fn render_send_pick(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let title = self
            .send_browser
            .as_ref()
            .map(|browser| browser.dir.display().to_string())
            .unwrap_or_else(|| strings.send_title.to_owned());
        let status = if self.send_selection.is_empty() {
            None
        } else {
            Some(fill(
                strings.send_selected_count,
                &self.send_selection.len().to_string(),
            ))
        };
        let content = ui::header(canvas, fonts, &title, status.as_deref());

        let rows = self.send_pick_rows();
        self.send_pick_cursor.clamp(rows.len());
        self.send_pick_cursor.scroll = ui::list(
            canvas,
            fonts,
            content,
            &rows,
            Some(self.send_pick_cursor.index),
            self.send_pick_cursor.scroll,
            strings.list_empty,
        );

        ui::footer(
            canvas,
            fonts,
            &[
                ("A", strings.hint_pick),
                ("Y", strings.hint_pick_folder),
                ("X", strings.hint_continue),
                ("B", strings.hint_back),
            ],
        );
    }

    fn render_send_to(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let status = fill(
            strings.send_selected_count,
            &self.send_selection.len().to_string(),
        );
        let content = ui::header(canvas, fonts, strings.send_to_title, Some(&status));

        let rows = self.send_to_rows();
        self.send_to_cursor.clamp(rows.len());
        self.send_to_cursor.scroll = ui::list(
            canvas,
            fonts,
            content,
            &rows,
            Some(self.send_to_cursor.index),
            self.send_to_cursor.scroll,
            strings.list_empty,
        );

        ui::footer(
            canvas,
            fonts,
            &[
                ("A", strings.hint_select),
                ("Y", strings.hint_rescan),
                ("B", strings.hint_back),
            ],
        );
    }

    fn render_send_code(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.send_code_title, None);

        // The code being typed, shown at the size the other device shows it.
        let entry = Rect::new(content.x, content.y, content.w, 140);
        ui::panel(canvas, entry);
        let typed: String = self
            .code_input
            .chars()
            .chain(std::iter::repeat('_'))
            .take(CODE_LENGTH)
            .flat_map(|ch| [ch, ' '])
            .collect();
        fonts.draw_in(
            canvas,
            Rect::new(entry.x, entry.y + 20, entry.w, 80),
            typed.trim_end(),
            text::CODE,
            theme::TEXT,
            Weight::Bold,
            Align::Center,
        );
        fonts.draw_in(
            canvas,
            Rect::new(entry.x, entry.bottom() - 34, entry.w, 26),
            strings.send_code_hint,
            text::SMALL,
            theme::TEXT_FAINT,
            Weight::Regular,
            Align::Center,
        );

        // Keyboard grid.
        let grid_top = entry.bottom() + metrics::GAP;
        let cell = (content.w / CODE_COLUMNS as i32).min(76);
        let rows = CODE_KEYS.len().div_ceil(CODE_COLUMNS) as i32;
        let grid_w = cell * CODE_COLUMNS as i32;
        let origin_x = content.x + (content.w - grid_w) / 2;

        for (index, key) in CODE_KEYS.iter().enumerate() {
            let column = (index % CODE_COLUMNS) as i32;
            let row = (index / CODE_COLUMNS) as i32;
            let rect = Rect::new(
                origin_x + column * cell,
                grid_top + row * cell,
                cell - 6,
                cell - 6,
            );
            let selected = index == self.code_key;
            canvas.fill_round_rect(
                rect,
                10,
                if selected {
                    theme::ACCENT_STRONG
                } else {
                    theme::SURFACE
                },
            );
            if !selected {
                canvas.stroke_rect(rect, 1, theme::BORDER);
            }
            fonts.draw_in(
                canvas,
                Rect::new(rect.x, rect.y + (rect.h - 30) / 2, rect.w, 32),
                &key.to_string(),
                text::HEADING,
                if selected {
                    theme::ON_ACCENT
                } else {
                    theme::TEXT
                },
                Weight::Bold,
                Align::Center,
            );
        }
        let _ = rows;

        ui::footer(
            canvas,
            fonts,
            &[
                ("A", strings.hint_type),
                ("X", strings.hint_delete),
                ("Start", strings.hint_send_now),
                ("B", strings.hint_back),
            ],
        );
    }

    fn render_send_progress(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let content = ui::header(canvas, fonts, strings.send_progress_title, None);
        let Some(event) = self.send_event.clone() else {
            return;
        };

        let panel_rect = Rect::new(content.x, content.y, content.w, 260);
        ui::panel(canvas, panel_rect);
        let inner = panel_rect.inset(metrics::GAP + 6);

        fonts.draw_in(
            canvas,
            Rect::new(inner.x, inner.y, inner.w, 36),
            &event.destination_label,
            text::HEADING,
            theme::TEXT,
            Weight::Bold,
            Align::Left,
        );
        fonts.draw_in(
            canvas,
            Rect::new(
                inner.x,
                inner.y + fonts.line_height(text::HEADING) + 6,
                inner.w,
                30,
            ),
            &event.status_message,
            text::BODY,
            theme::TEXT_MUTED,
            Weight::Regular,
            Align::Left,
        );

        // Hashing runs before a byte moves and is slow on a big file, so it
        // gets its own fraction rather than sitting at 0%.
        let (done, total) = match (event.bytes_hashed, event.phase) {
            (Some(hashed), SendPhase::Preparing) => (hashed, event.total_size),
            _ => (event.bytes_sent, event.total_size),
        };
        let fraction = if total > 0 {
            done as f32 / total as f32
        } else {
            0.0
        };
        let bar = Rect::new(inner.x, inner.y + 120, inner.w, 16);
        ui::progress_bar(canvas, bar, fraction, theme::ACCENT_STRONG);

        fonts.draw(
            canvas,
            inner.x,
            bar.bottom() + 12,
            &format!("{} / {}", human_size(done), human_size(total)),
            text::BODY,
            theme::TEXT,
            Weight::Regular,
        );
        let percent = format!("{}%", (fraction * 100.0).round() as i32);
        fonts.draw_in(
            canvas,
            Rect::new(inner.x, bar.y - 48, inner.w, 44),
            &percent,
            text::TITLE,
            theme::TEXT,
            Weight::Bold,
            Align::Right,
        );

        ui::footer(canvas, fonts, &[("B", strings.hint_cancel)]);
    }

    fn render_send_result(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let strings = self.s();
        let Some(event) = self.send_event.clone() else {
            self.screen = Screen::Home;
            return;
        };

        let (title, color, headline) = match event.phase {
            SendPhase::Completed => (
                strings.send_completed,
                theme::SUCCESS,
                fill2(
                    plural(
                        event.item_count,
                        strings.send_sent_summary_one,
                        strings.send_sent_summary,
                    ),
                    &event.item_count.to_string(),
                    &human_size(event.total_size),
                ),
            ),
            SendPhase::Declined => (
                strings.send_declined,
                theme::TEXT_MUTED,
                strings.send_declined_body.to_owned(),
            ),
            SendPhase::Cancelled => (
                strings.send_cancelled,
                theme::WARNING,
                strings.cancelled_body.to_owned(),
            ),
            _ => (
                strings.send_failed,
                theme::DANGER,
                event
                    .error
                    .as_ref()
                    .map(|error| error.title().to_owned())
                    .unwrap_or_else(|| event.status_message.clone()),
            ),
        };

        let content = ui::header(canvas, fonts, title, None);
        let head = Rect::new(content.x, content.y, content.w, 120);
        ui::panel(canvas, head);
        let inner = head.inset(metrics::GAP);
        fonts.draw_in(
            canvas,
            Rect::new(inner.x, inner.y, inner.w, 36),
            &headline,
            text::HEADING,
            color,
            Weight::Bold,
            Align::Left,
        );
        let detail = match event.phase {
            SendPhase::Completed => event.destination_label.clone(),
            _ => event
                .error
                .as_ref()
                .map(|error| error.message().to_owned())
                .unwrap_or_else(|| event.destination_label.clone()),
        };
        fonts.draw_in(
            canvas,
            Rect::new(
                inner.x,
                inner.y + fonts.line_height(text::HEADING) + 6,
                inner.w,
                30,
            ),
            &detail,
            text::BODY,
            theme::TEXT_MUTED,
            Weight::Regular,
            Align::Left,
        );

        ui::footer(
            canvas,
            fonts,
            &[("A", strings.hint_done), ("X", strings.hint_other_device)],
        );
    }

    fn render_toast(&mut self, canvas: &mut Canvas, fonts: &mut Fonts) {
        let Some(toast) = self.toast.clone() else {
            return;
        };
        if Instant::now() >= toast.until {
            self.toast = None;
            return;
        }
        let width = fonts.measure(&toast.message, text::BODY) + metrics::GAP * 3;
        let height = fonts.line_height(text::BODY) + 24;
        let rect = Rect::new(
            (canvas.width() - width) / 2,
            canvas.height() - metrics::FOOTER_HEIGHT - height - metrics::GAP,
            width,
            height,
        );
        canvas.fill_round_rect(rect, height / 2, theme::SURFACE_RAISED);
        canvas.stroke_rect(rect, 1, theme::BORDER);
        fonts.draw_in(
            canvas,
            Rect::new(rect.x, rect.y + 12, rect.w, height),
            &toast.message,
            text::BODY,
            theme::TEXT,
            Weight::Regular,
            Align::Center,
        );
    }
}

/// Turns the rendezvous expiry into a countdown.
///
/// The server sends an RFC3339 UTC instant
/// (`2026-09-27T05:07:25.470803654Z`). Rendered literally it is unreadable at
/// arm's length and wide enough to overrun the code panel, which is exactly
/// what it did on the device. A value that will not parse yields `None` rather
/// than a line that might be wrong.
fn format_expiry(expires_at: &str, now: OffsetDateTime, strings: &Strings) -> Option<String> {
    let expiry = OffsetDateTime::parse(expires_at, &Rfc3339).ok()?;
    let remaining = expiry - now;
    if remaining <= time::Duration::ZERO {
        return Some(strings.code_expired.to_owned());
    }
    let seconds = remaining.whole_seconds();
    Some(if seconds >= 90 {
        // Ceiling, so the countdown never reads "0 min" while time is left.
        let minutes = seconds.div_euclid(60) + i64::from(seconds % 60 != 0);
        fill(strings.minutes_left, &minutes.to_string())
    } else {
        fill(strings.seconds_left, &seconds.to_string())
    })
}

/// `K7M2Q9` -> `K7M 2Q9`, matching how the phone app groups the code.
fn spaced_code(code: &str) -> String {
    let chars: Vec<char> = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if chars.len() != 6 {
        return code.to_owned();
    }
    let first: String = chars[..3].iter().collect();
    let second: String = chars[3..].iter().collect();
    format!("{first} {second}")
}

/// Shortens a base32 endpoint id to something that fits a list row.
fn short_key(key: &str) -> String {
    if key.len() <= 16 {
        return key.to_owned();
    }
    format!("{}…{}", &key[..8], &key[key.len() - 6..])
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Conflict;
    use crate::engine::EngineEvent;
    use crate::i18n::Lang;

    fn press(button: Button) -> KeyEvent {
        KeyEvent {
            button: Some(button),
            pressed: true,
            repeat: false,
            raw_type: 1,
            raw_code: 0,
            raw_value: 1,
        }
    }

    fn offer(phase: ReceiverOfferPhase) -> ReceiverOfferEvent {
        ReceiverOfferEvent {
            phase,
            sender_name: "Pixel 7".to_owned(),
            sender_device_type: "phone".to_owned(),
            sender_web: false,
            sender_ephemeral: false,
            destination_label: "/mnt/SDCARD/Wisp".to_owned(),
            save_root_label: "Wisp".to_owned(),
            status_message: String::new(),
            item_count: 2,
            total_size_bytes: 2048,
            bytes_received: 0,
            plan: None,
            snapshot: None,
            connection_path: None,
            sender_endpoint_id: Some("sender-key".to_owned()),
            sender_ticket: None,
            total_size_label: "2 KB".to_owned(),
            files: Vec::new(),
            inline_text: None,
            error: None,
        }
    }

    fn app() -> App {
        App::new(Config::default())
    }

    fn is_accept(request: &AppRequest) -> bool {
        matches!(
            request,
            AppRequest::Engine(EngineCommand::Respond(OfferDecision::Accept(_)))
        )
    }

    #[test]
    fn an_untrusted_offer_waits_for_the_user() {
        let mut app = app();
        let requests = app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        assert_eq!(app.screen(), Screen::Offer);
        assert!(requests.is_empty(), "nothing may be answered automatically");
    }

    #[test]
    fn a_trusted_sender_is_accepted_without_a_prompt() {
        let mut app = app();
        app.config.trust("sender-key", "Pixel 7");
        let requests = app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        assert_eq!(app.screen(), Screen::Transfer);
        assert!(requests.iter().any(is_accept));
    }

    #[test]
    fn an_ephemeral_sender_is_never_auto_accepted() {
        let mut app = app();
        app.config.trust("sender-key", "Browser");
        let mut event = offer(ReceiverOfferPhase::OfferReady);
        // A browser or CLI peer has a throwaway key; trusting it once must not
        // silently accept a different session that happens to reuse the id.
        event.sender_ephemeral = true;
        let requests = app.handle_engine(EngineEvent::Offer(event));
        assert_eq!(app.screen(), Screen::Offer);
        assert!(requests.is_empty());
    }

    #[test]
    fn a_repeated_offer_ready_does_not_accept_twice() {
        let mut app = app();
        app.config.trust("sender-key", "Pixel 7");
        let first = app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        let second = app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        assert_eq!(first.iter().filter(|r| is_accept(r)).count(), 1);
        assert!(second.iter().all(|r| !is_accept(r)));
    }

    #[test]
    fn accepting_from_the_offer_screen_asks_the_engine() {
        let mut app = app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        let requests = app.handle_key(press(Button::A));
        assert!(requests.iter().any(is_accept));
        assert_eq!(app.screen(), Screen::Transfer);
    }

    #[test]
    fn declining_sends_a_decline() {
        let mut app = app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        let requests = app.handle_key(press(Button::B));
        assert!(requests.iter().any(|r| matches!(
            r,
            AppRequest::Engine(EngineCommand::Respond(OfferDecision::Decline))
        )));
    }

    #[test]
    fn trust_and_accept_records_the_sender() {
        let mut app = app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        let requests = app.handle_key(press(Button::X));
        assert!(app.config.is_trusted("sender-key"));
        assert!(requests.iter().any(is_accept));
        assert!(requests.iter().any(|r| matches!(r, AppRequest::SaveConfig)));
    }

    #[test]
    fn a_key_release_never_triggers_an_action() {
        let mut app = app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        let release = KeyEvent {
            pressed: false,
            ..press(Button::A)
        };
        assert!(app.handle_key(release).is_empty());
        assert_eq!(app.screen(), Screen::Offer, "still waiting for a decision");
    }

    #[test]
    fn a_terminal_phase_moves_to_the_result_screen() {
        let mut app = app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::Completed)));
        assert_eq!(app.screen(), Screen::Result);
        app.handle_key(press(Button::A));
        assert_eq!(app.screen(), Screen::Home);
    }

    #[test]
    fn cancelling_a_transfer_asks_the_engine() {
        let mut app = app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::Receiving)));
        assert_eq!(app.screen(), Screen::Transfer);
        let requests = app.handle_key(press(Button::B));
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, AppRequest::Engine(EngineCommand::Cancel)))
        );
    }

    #[test]
    fn home_exits_on_b_and_opens_settings_on_y() {
        let mut app = app();
        assert!(matches!(
            app.handle_key(press(Button::Y)).as_slice(),
            [] if app.screen() == Screen::Settings
        ));
        app.handle_key(press(Button::B));
        assert_eq!(app.screen(), Screen::Home);
        let requests = app.handle_key(press(Button::B));
        assert!(requests.iter().any(|r| matches!(r, AppRequest::Exit)));
    }

    #[test]
    fn toggling_the_conflict_policy_restarts_the_receiver() {
        let mut app = app();
        let mut requests = Vec::new();
        app.handle_key(press(Button::Y));
        for _ in 0..SettingsRow::ORDER
            .iter()
            .position(|r| *r == SettingsRow::Conflict)
            .unwrap()
        {
            app.handle_key(press(Button::Down));
        }
        requests.extend(app.handle_key(press(Button::A)));
        assert_eq!(app.config.conflict, Conflict::Reject);
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, AppRequest::RestartEngine))
        );
    }

    fn send_event(phase: SendPhase) -> SendEvent {
        SendEvent {
            phase,
            destination_label: "Pixel 7".to_owned(),
            status_message: String::new(),
            item_count: 2,
            total_size: 2048,
            bytes_sent: 0,
            plan: None,
            snapshot: None,
            remote_device_type: None,
            remote_endpoint_id: Some("peer-key".to_owned()),
            remote_ephemeral: Some(false),
            remote_ticket: Some("peer-ticket".to_owned()),
            bytes_hashed: None,
            connection_path: None,
            connection_candidates: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn x_on_home_opens_the_send_picker() {
        let mut app = app();
        app.handle_key(press(Button::X));
        assert_eq!(app.screen(), Screen::SendPick);
    }

    #[test]
    fn continuing_with_nothing_picked_does_not_advance() {
        let mut app = app();
        app.handle_key(press(Button::X));
        let requests = app.handle_key(press(Button::X));
        assert_eq!(app.screen(), Screen::SendPick, "still picking");
        assert!(requests.is_empty());
    }

    #[test]
    fn continuing_with_a_selection_scans_for_destinations() {
        let mut app = app();
        app.handle_key(press(Button::X));
        app.send_selection.push(PathBuf::from("/tmp/a.bin"));
        let requests = app.handle_key(press(Button::X));

        assert_eq!(app.screen(), Screen::SendTo);
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, AppRequest::Engine(EngineCommand::ScanNearby { .. })))
        );
    }

    #[test]
    fn nearby_results_and_recent_devices_both_become_targets() {
        let mut app = app();
        app.config
            .remember_device("key-old", "MacBook", "ticket-old");
        app.handle_engine(EngineEvent::Nearby(Ok(vec![NearbyReceiver {
            fullname: "pixel".to_owned(),
            label: "Pixel 7".to_owned(),
            device_type: "phone".to_owned(),
            code: String::new(),
            ticket: "ticket-new".to_owned(),
            endpoint_id: "key-new".to_owned(),
            over_usb: false,
        }])));

        let labels: Vec<String> = app
            .send_targets()
            .iter()
            .map(|t| t.label().to_owned())
            .collect();
        assert_eq!(labels, vec!["Pixel 7", "MacBook"], "nearby leads");
    }

    #[test]
    fn a_device_found_nearby_is_not_listed_twice() {
        let mut app = app();
        app.config.remember_device("key", "Pixel 7", "same-ticket");
        app.handle_engine(EngineEvent::Nearby(Ok(vec![NearbyReceiver {
            fullname: "pixel".to_owned(),
            label: "Pixel 7".to_owned(),
            device_type: "phone".to_owned(),
            code: String::new(),
            ticket: "same-ticket".to_owned(),
            endpoint_id: "key".to_owned(),
            over_usb: false,
        }])));
        assert_eq!(app.send_targets().len(), 1);
    }

    #[test]
    fn the_code_keyboard_types_deletes_and_only_sends_when_complete() {
        let mut app = app();
        app.screen = Screen::SendCode;

        // '0' is the first key; move right twice to reach '2'.
        app.handle_key(press(Button::A));
        app.handle_key(press(Button::Right));
        app.handle_key(press(Button::A));
        assert_eq!(app.code_input, "01");

        app.handle_key(press(Button::X));
        assert_eq!(app.code_input, "0");

        // Start does nothing until six characters are in.
        assert!(app.handle_key(press(Button::Start)).is_empty());
        assert_eq!(app.screen(), Screen::SendCode);

        while app.code_input.chars().count() < CODE_LENGTH {
            app.handle_key(press(Button::A));
        }
        let requests = app.handle_key(press(Button::Start));
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, AppRequest::Engine(EngineCommand::StartSend { .. })))
        );
        assert_eq!(app.screen(), Screen::SendProgress);
    }

    #[test]
    fn the_keyboard_cursor_stays_inside_the_grid() {
        let mut app = app();
        app.screen = Screen::SendCode;
        for _ in 0..40 {
            app.handle_key(press(Button::Right));
            app.handle_key(press(Button::Down));
        }
        assert!(app.code_key < CODE_KEYS.len());
        for _ in 0..40 {
            app.handle_key(press(Button::Left));
            app.handle_key(press(Button::Up));
        }
        assert_eq!(app.code_key, 0);
    }

    #[test]
    fn a_completed_send_is_remembered_for_next_time() {
        let mut app = app();
        let requests = app.handle_engine(EngineEvent::Send(send_event(SendPhase::Completed)));
        assert_eq!(app.screen(), Screen::SendResult);
        assert_eq!(app.config.recent_devices.len(), 1);
        assert_eq!(app.config.recent_devices[0].ticket, "peer-ticket");
        assert!(requests.iter().any(|r| matches!(r, AppRequest::SaveConfig)));
    }

    #[test]
    fn a_failed_send_is_not_remembered() {
        let mut app = app();
        app.handle_engine(EngineEvent::Send(send_event(SendPhase::Failed)));
        assert_eq!(app.screen(), Screen::SendResult);
        assert!(app.config.recent_devices.is_empty());
    }

    #[test]
    fn an_ephemeral_receiver_is_not_offered_again() {
        let mut app = app();
        let mut event = send_event(SendPhase::Completed);
        // A browser receiver's key is thrown away when the tab closes, so the
        // ticket would never dial twice.
        event.remote_ephemeral = Some(true);
        app.handle_engine(EngineEvent::Send(event));
        assert!(app.config.recent_devices.is_empty());
    }

    #[test]
    fn cancelling_a_send_asks_the_engine() {
        let mut app = app();
        app.handle_engine(EngineEvent::Send(send_event(SendPhase::Sending)));
        assert_eq!(app.screen(), Screen::SendProgress);
        let requests = app.handle_key(press(Button::B));
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, AppRequest::Engine(EngineCommand::CancelSend)))
        );
    }

    #[test]
    fn every_send_screen_renders_in_both_languages() {
        let mut fonts = Fonts::load().unwrap();
        let mut canvas = Canvas::new(1024, 768);
        for lang in [Lang::En, Lang::Vi] {
            for phase in [
                SendPhase::Preparing,
                SendPhase::Sending,
                SendPhase::Completed,
                SendPhase::Declined,
                SendPhase::Failed,
            ] {
                let mut app = app();
                app.config.lang = lang;
                app.handle_engine(EngineEvent::Send(send_event(phase)));
                app.render(&mut canvas, &mut fonts);
            }

            let mut app = app();
            app.config.lang = lang;
            app.config.remember_device("k", "Laptop", "t");
            for screen in [Screen::SendPick, Screen::SendTo, Screen::SendCode] {
                app.screen = screen;
                app.render(&mut canvas, &mut fonts);
            }
        }
    }

    #[test]
    fn about_is_the_last_settings_row_and_opens() {
        assert_eq!(
            SettingsRow::ORDER.last(),
            Some(&SettingsRow::About),
            "About belongs at the end of the menu"
        );
        let mut app = app();
        open_settings_row(&mut app, SettingsRow::About);
        assert_eq!(app.screen(), Screen::About);

        app.handle_key(press(Button::B));
        assert_eq!(app.screen(), Screen::Settings);
    }

    #[test]
    fn choosing_a_folder_records_it_and_keeps_the_rows_aligned() {
        // Use a directory that really exists, since the picker filters recent
        // entries by presence on disk.
        let dir = std::env::temp_dir();
        let mut app = app();
        app.config.remember_save_root(&dir);

        let candidates = app.folder_candidates();
        assert_eq!(
            candidates
                .first()
                .map(|(path, recent)| (path.clone(), *recent)),
            Some((dir.clone(), true)),
            "a remembered folder leads the list"
        );
        // One row per candidate, plus the trailing "Browse…" entry, or the
        // index the handler uses would point at the wrong folder.
        assert_eq!(app.folder_rows().len(), candidates.len() + 1);
    }

    #[test]
    fn switching_language_persists_but_leaves_the_receiver_alone() {
        let mut app = app();
        assert_eq!(app.config.lang, Lang::En, "English is the default");

        open_settings_row(&mut app, SettingsRow::Language);
        assert_eq!(app.config.lang, Lang::Vi);
        assert_eq!(app.screen(), Screen::Settings, "stays on the menu");

        // Re-open to collect the requests from the second toggle.
        let mut app = app;
        let requests = app.handle_key(press(Button::A));
        assert_eq!(app.config.lang, Lang::En, "toggles back");
        assert!(requests.iter().any(|r| matches!(r, AppRequest::SaveConfig)));
        assert!(
            !requests
                .iter()
                .any(|r| matches!(r, AppRequest::RestartEngine)),
            "language is drawn, not served — no need to rebuild the receiver"
        );
    }

    #[test]
    fn the_whole_ui_renders_in_both_languages() {
        let mut fonts = Fonts::load().unwrap();
        let mut canvas = Canvas::new(1024, 768);
        for lang in [Lang::En, Lang::Vi] {
            let mut app = app();
            app.config.lang = lang;
            app.config.trust("key-a", "Laptop");
            for screen in [
                Screen::Home,
                Screen::Settings,
                Screen::SaveFolder,
                Screen::Trusted,
                Screen::About,
                Screen::ButtonTest,
            ] {
                app.screen = screen;
                app.render(&mut canvas, &mut fonts);
            }
        }
    }

    /// Walks the settings cursor to `row` rather than pressing Down a fixed
    /// number of times, so inserting a settings row cannot silently make this
    /// test navigate somewhere else.
    fn open_settings_row(app: &mut App, row: SettingsRow) {
        app.handle_key(press(Button::Y));
        let target = SettingsRow::ORDER
            .iter()
            .position(|candidate| *candidate == row)
            .expect("row must be in ORDER");
        for _ in 0..target {
            app.handle_key(press(Button::Down));
        }
        app.handle_key(press(Button::A));
    }

    #[test]
    fn removing_a_trusted_device_persists() {
        let mut app = app();
        app.config.trust("key-a", "Laptop");
        open_settings_row(&mut app, SettingsRow::Trusted);
        assert_eq!(app.screen(), Screen::Trusted);

        let requests = app.handle_key(press(Button::X));
        assert!(app.config.trusted.is_empty());
        assert!(requests.iter().any(|r| matches!(r, AppRequest::SaveConfig)));
    }

    #[test]
    fn the_cursor_stops_at_both_ends() {
        let mut cursor = Cursor::default();
        cursor.move_by(-1, 3);
        assert_eq!(cursor.index, 0);
        cursor.move_by(9, 3);
        assert_eq!(cursor.index, 2);
    }

    #[test]
    fn the_cursor_survives_a_list_shrinking_under_it() {
        let mut cursor = Cursor {
            index: 7,
            scroll: 0,
        };
        cursor.clamp(2);
        assert_eq!(cursor.index, 1);
        cursor.clamp(0);
        assert_eq!(cursor.index, 0);
    }

    #[test]
    fn a_fatal_engine_error_is_shown_instead_of_the_home_screen() {
        let mut app = app();
        app.handle_engine(EngineEvent::Fatal("cổng đã bị chiếm".to_owned()));
        let mut canvas = Canvas::new(1024, 768);
        let mut fonts = Fonts::load().unwrap();
        // Rendering must not panic, and must not depend on a receiver.
        app.render(&mut canvas, &mut fonts);
    }

    #[test]
    fn a_long_fatal_message_still_fits_inside_its_panel() {
        // The unmounted-card explanation runs several paragraphs; a fixed
        // panel height clipped its last line on the device.
        let long = "/mnt/SDCARD/Wisp is not on a mounted card.\n\n\
             The SD card is missing or busy. Anything received now would be \
             written to internal storage and hidden as soon as the card \
             mounts again, so nothing will be accepted.\n\n\
             Reseat the card or restart the device, then open Wisp again.";
        let mut app = app();
        app.set_fatal(long);

        let mut fonts = Fonts::load().unwrap();
        let mut canvas = Canvas::new(1024, 768);
        app.render(&mut canvas, &mut fonts);

        // Nothing may be drawn in the gap just above the footer.
        let pixels = canvas.pixels();
        let footer_top = (768 - metrics::FOOTER_HEIGHT) as usize;
        let mut stray = 0;
        for y in (footer_top - 4)..footer_top {
            for x in 0..1024 {
                if pixels[y * 1024 + x] != theme::BG {
                    stray += 1;
                }
            }
        }
        assert_eq!(stray, 0, "the panel must not spill into the footer");
    }

    #[test]
    fn every_screen_renders_without_panicking() {
        let mut fonts = Fonts::load().unwrap();
        let mut canvas = Canvas::new(1024, 768);

        for phase in [
            ReceiverOfferPhase::Connecting,
            ReceiverOfferPhase::OfferReady,
            ReceiverOfferPhase::Receiving,
            ReceiverOfferPhase::Completed,
            ReceiverOfferPhase::Failed,
        ] {
            let mut app = app();
            let mut event = offer(phase);
            event.files = (0..30)
                .map(|i| wisp_app::ReceiverOfferFile {
                    path: format!("folder/file-{i}.bin"),
                    size: 1024 * i,
                })
                .collect();
            app.handle_engine(EngineEvent::Offer(event));
            app.render(&mut canvas, &mut fonts);
        }

        let mut app = app();
        app.config.trust("key-a", "Laptop");
        for screen in [
            Screen::Home,
            Screen::Settings,
            Screen::SaveFolder,
            Screen::Trusted,
            Screen::About,
            Screen::ButtonTest,
        ] {
            app.screen = screen;
            app.render(&mut canvas, &mut fonts);
        }
    }

    #[test]
    fn a_text_offer_renders_its_body() {
        let mut app = app();
        let mut event = offer(ReceiverOfferPhase::Completed);
        event.inline_text = Some("https://example.com\nmột dòng nữa".to_owned());
        event.item_count = 0;
        app.handle_engine(EngineEvent::Offer(event));

        let mut canvas = Canvas::new(1024, 768);
        let mut fonts = Fonts::load().unwrap();
        app.render(&mut canvas, &mut fonts);
        assert_eq!(app.screen(), Screen::Result);
    }

    /// The literal string the rendezvous server returned on-device, which is
    /// what broke the panel layout in the first place.
    const REAL_EXPIRY: &str = "2026-09-27T05:07:25.470803654Z";

    fn at_utc(text: &str) -> OffsetDateTime {
        OffsetDateTime::parse(text, &Rfc3339).unwrap()
    }

    #[test]
    fn expiry_becomes_a_short_countdown_in_both_languages() {
        let now = at_utc("2026-09-27T05:02:25Z");
        assert_eq!(
            format_expiry(REAL_EXPIRY, now, Lang::En.strings()).as_deref(),
            Some("5 min left")
        );
        assert_eq!(
            format_expiry(REAL_EXPIRY, now, Lang::Vi.strings()).as_deref(),
            Some("Còn 5 phút")
        );
    }

    #[test]
    fn expiry_switches_to_seconds_near_the_end() {
        let now = at_utc("2026-09-27T05:07:00Z");
        assert_eq!(
            format_expiry(REAL_EXPIRY, now, Lang::En.strings()).as_deref(),
            Some("25 s left")
        );
    }

    #[test]
    fn a_passed_expiry_says_so() {
        let now = at_utc("2026-09-27T06:00:00Z");
        assert_eq!(
            format_expiry(REAL_EXPIRY, now, Lang::En.strings()).as_deref(),
            Some("Code expired — press A")
        );
    }

    #[test]
    fn an_unparseable_expiry_shows_nothing_rather_than_something_wrong() {
        let now = at_utc("2026-09-27T05:02:25Z");
        let strings = Lang::En.strings();
        assert_eq!(format_expiry("soon", now, strings), None);
        assert_eq!(format_expiry("", now, strings), None);
    }

    #[test]
    fn the_countdown_never_gets_wide_enough_to_overrun_the_panel() {
        // The bug this replaces rendered a 30-character timestamp into a
        // panel sized for a short hint.
        let now = at_utc("2026-09-27T05:02:25Z");
        for lang in [Lang::En, Lang::Vi] {
            let text = format_expiry(REAL_EXPIRY, now, lang.strings()).unwrap();
            assert!(text.chars().count() <= 24, "{text:?} is too wide");
        }
    }

    #[test]
    fn code_formatting_groups_six_characters() {
        assert_eq!(spaced_code("K7M2Q9"), "K7M 2Q9");
        assert_eq!(spaced_code("short"), "short");
    }

    #[test]
    fn long_keys_are_shortened_for_display() {
        let key = "a".repeat(52);
        let shown = short_key(&key);
        assert!(shown.len() < key.len());
        assert!(shown.contains('…'));
        assert_eq!(short_key("abc"), "abc");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(9), "9s");
        assert_eq!(format_duration(75), "1m15s");
        assert_eq!(format_duration(3_725), "1h02m");
    }
}
