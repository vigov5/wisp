//! Renders every screen to a PNG so the layout can be reviewed without the
//! handheld.
//!
//! ```sh
//! cargo run -p wisp-trimui --example preview -- target/preview
//! ```
//!
//! The PNG writer below is deliberately dependency-free: it emits stored
//! (uncompressed) deflate blocks, which is all a review image needs and is
//! cheaper than pulling an image crate into the build for a dev tool.

use std::path::{Path, PathBuf};

use wisp_app::{
    PairingCodeState, QrPairingInfo, ReceiverOfferEvent, ReceiverOfferFile, ReceiverOfferPhase,
    ReceiverRegistration,
};
use wisp_trimui::app::App;
use wisp_trimui::config::Config;
use wisp_trimui::draw::Canvas;
use wisp_trimui::engine::EngineEvent;
use wisp_trimui::font::Fonts;
use wisp_trimui::input::{Button, KeyEvent};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;

fn main() -> anyhow::Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/preview"));
    std::fs::create_dir_all(&out_dir)?;

    let mut fonts = Fonts::load()?;
    let mut canvas = Canvas::new(WIDTH, HEIGHT);

    for (name, mut app) in scenes() {
        app.render(&mut canvas, &mut fonts);
        let path = out_dir.join(format!("{name}.png"));
        write_png(&path, WIDTH, HEIGHT, canvas.pixels())?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn press(button: Button) -> KeyEvent {
    KeyEvent {
        button: Some(button),
        pressed: true,
        repeat: false,
        raw_type: 1,
        raw_code: 305,
        raw_value: 1,
    }
}

fn base_app() -> App {
    let mut config = Config::default();
    config.device_name = "Brick Pro".to_owned();
    config.trust("k1", "Pixel 7");
    config.trust("k2", "MacBook Air");
    let mut app = App::new(config);
    app.handle_engine(EngineEvent::Ready {
        // Shaped like a real base32 endpoint id so the shortened form on the
        // home screen is the width it will really be.
        endpoint_id: "5b547fc97b50fe94997b6ef827d3eb246e0f7180693b95d432475d825feaa51c".to_owned(),
    });
    app.handle_engine(EngineEvent::Code(PairingCodeState::Active(
        ReceiverRegistration {
            code: "K7M2Q9".to_owned(),
            // The rendezvous server really does send RFC3339, and the home
            // screen turns it into a countdown; a placeholder that will not
            // parse would hide the line the layout has to fit.
            expires_at: (time::OffsetDateTime::now_utc() + time::Duration::minutes(5))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        },
    )));
    app.handle_engine(EngineEvent::Pairing(QrPairingInfo {
        // Stand-in for a real ticket: same shape, same rough length, so the
        // QR density in the preview matches what the device will show.
        ticket: format!("wisp-pair:{}", "eyJ0aWNrZXQiOiJ4".repeat(14)),
        lan_ips: vec!["192.168.1.47:41234".to_owned()],
    }));
    app
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
        item_count: 4,
        total_size_bytes: 148 * 1024 * 1024,
        bytes_received: 0,
        plan: None,
        snapshot: None,
        connection_path: None,
        sender_endpoint_id: Some("k9".to_owned()),
        sender_ticket: None,
        total_size_label: "148 MB".to_owned(),
        files: vec![
            ReceiverOfferFile {
                path: "Roms/GBA/Metroid Fusion.gba".to_owned(),
                size: 8 * 1024 * 1024,
            },
            ReceiverOfferFile {
                path: "Roms/GBA/Golden Sun.gba".to_owned(),
                size: 16 * 1024 * 1024,
            },
            ReceiverOfferFile {
                path: "Roms/SNES/Chrono Trigger.sfc".to_owned(),
                size: 4 * 1024 * 1024,
            },
            ReceiverOfferFile {
                path: "Roms/PS1/Final Fantasy VII (Disc 1).chd".to_owned(),
                size: 120 * 1024 * 1024,
            },
        ],
        inline_text: None,
        error: None,
    }
}

fn scenes() -> Vec<(&'static str, App)> {
    let mut scenes: Vec<(&'static str, App)> = Vec::new();

    scenes.push(("home", base_app()));

    {
        // The same screen in the other language: the two must both fit, and
        // Vietnamese runs noticeably longer than English.
        let mut app = base_app();
        app.config.lang = wisp_trimui::i18n::Lang::Vi;
        scenes.push(("home-vi", app));
    }

    {
        let mut app = base_app();
        app.config.lang = wisp_trimui::i18n::Lang::Vi;
        app.handle_key(press(Button::Y));
        scenes.push(("settings-vi", app));
    }

    {
        let mut app = base_app();
        app.handle_engine(EngineEvent::Code(PairingCodeState::Unavailable));
        app.handle_engine(EngineEvent::CodeUnavailable("no route to host".to_owned()));
        scenes.push(("home-offline", app));
    }

    {
        let mut app = base_app();
        app.handle_engine(EngineEvent::Offer(offer(ReceiverOfferPhase::OfferReady)));
        scenes.push(("offer", app));
    }

    {
        let mut app = base_app();
        let mut event = offer(ReceiverOfferPhase::Receiving);
        event.bytes_received = 61 * 1024 * 1024;
        event.status_message = "Đang nhận Final Fantasy VII (Disc 1).chd".to_owned();
        event.snapshot = Some(wisp_app::TransferSnapshot {
            session_id: "s1".to_owned(),
            phase: wisp_core::transfer::TransferPhase::Transferring,
            total_files: 4,
            completed_files: 2,
            total_bytes: 148 * 1024 * 1024,
            bytes_transferred: 61 * 1024 * 1024,
            active_file_id: None,
            active_file_bytes: None,
            bytes_per_sec: Some(9 * 1024 * 1024),
            eta_seconds: Some(9),
        });
        app.handle_engine(EngineEvent::Offer(event));
        scenes.push(("transfer", app));
    }

    {
        let mut app = base_app();
        let mut event = offer(ReceiverOfferPhase::Completed);
        event.bytes_received = event.total_size_bytes;
        app.handle_engine(EngineEvent::Offer(event));
        scenes.push(("result", app));
    }

    {
        let mut app = base_app();
        let mut event = offer(ReceiverOfferPhase::Completed);
        event.item_count = 0;
        event.files.clear();
        event.total_size_bytes = 0;
        event.inline_text = Some(
            "https://github.com/vigov5/wisp\n\nGhi chú: bản port cho TrimUI Brick Pro \
             chỉ nhận, chưa gửi. Bấm A để quay lại màn hình chờ."
                .to_owned(),
        );
        app.handle_engine(EngineEvent::Offer(event));
        scenes.push(("result-text", app));
    }

    {
        let mut app = base_app();
        app.handle_key(press(Button::Y));
        scenes.push(("settings", app));
    }

    {
        let mut app = base_app();
        app.handle_key(press(Button::Y));
        app.handle_key(press(Button::A));
        scenes.push(("save-folder", app));
    }

    {
        let mut app = base_app();
        app.handle_key(press(Button::Y));
        app.handle_key(press(Button::Down));
        app.handle_key(press(Button::Down));
        app.handle_key(press(Button::A));
        scenes.push(("trusted", app));
    }

    {
        let mut app = base_app();
        app.handle_key(press(Button::Y));
        app.handle_key(press(Button::Down));
        app.handle_key(press(Button::Down));
        app.handle_key(press(Button::Down));
        app.handle_key(press(Button::A));
        scenes.push(("button-test", app));
    }

    {
        // Last row of Settings.
        let mut app = base_app();
        app.handle_key(press(Button::Y));
        for _ in 0..6 {
            app.handle_key(press(Button::Down));
        }
        app.handle_key(press(Button::A));
        scenes.push(("about", app));
    }

    // --- sending

    let queued = || {
        vec![
            std::path::PathBuf::from("/mnt/SDCARD/Roms/GBA/Metroid Fusion.gba"),
            std::path::PathBuf::from("/mnt/SDCARD/Screenshots/shot.png"),
        ]
    };

    {
        let mut app = base_app();
        app.handle_engine(EngineEvent::Nearby(Ok(vec![wisp_app::NearbyReceiver {
            fullname: "pixel".to_owned(),
            label: "Pixel 7".to_owned(),
            device_type: "phone".to_owned(),
            code: String::new(),
            ticket: "ticket-a".to_owned(),
            endpoint_id: "key-a".to_owned(),
            over_usb: false,
        }])));
        app.config
            .remember_device("key-b", "MacBook Air", "ticket-b");
        app.preview_send(wisp_trimui::app::Screen::SendTo, queued());
        scenes.push(("send-to", app));
    }

    {
        let mut app = base_app();
        app.preview_send(wisp_trimui::app::Screen::SendCode, queued());
        scenes.push(("send-code", app));
    }

    {
        let mut app = base_app();
        let mut event = send_progress_event();
        event.bytes_sent = 61 * 1024 * 1024;
        app.handle_engine(EngineEvent::Send(event));
        scenes.push(("send-progress", app));
    }

    {
        let mut app = base_app();
        let mut event = send_progress_event();
        event.phase = wisp_app::SendPhase::Completed;
        event.bytes_sent = event.total_size;
        app.handle_engine(EngineEvent::Send(event));
        scenes.push(("send-result", app));
    }

    scenes
}

fn send_progress_event() -> wisp_app::SendEvent {
    wisp_app::SendEvent {
        phase: wisp_app::SendPhase::Sending,
        destination_label: "Pixel 7".to_owned(),
        status_message: "Sending Metroid Fusion.gba".to_owned(),
        item_count: 2,
        total_size: 148 * 1024 * 1024,
        bytes_sent: 0,
        plan: None,
        snapshot: None,
        remote_device_type: Some("phone".to_owned()),
        remote_endpoint_id: Some("key-a".to_owned()),
        remote_ephemeral: Some(false),
        remote_ticket: Some("ticket-a".to_owned()),
        bytes_hashed: None,
        connection_path: None,
        connection_candidates: Vec::new(),
        error: None,
    }
}

// ------------------------------------------------------------------ PNG out

fn write_png(path: &Path, width: u32, height: u32, pixels: &[u32]) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity((width as usize * 3 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: none
        for x in 0..width as usize {
            let pixel = pixels[y * width as usize + x];
            raw.push((pixel >> 16) as u8);
            raw.push((pixel >> 8) as u8);
            raw.push(pixel as u8);
        }
    }

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour RGB
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    write_chunk(&mut out, b"IEND", &[]);

    std::fs::write(path, out)
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A zlib stream made only of stored deflate blocks — valid, and trivial.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(0xFFFF).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while let Some(chunk) = chunks.next() {
        let final_block = chunks.peek().is_none();
        out.push(if final_block { 1 } else { 0 });
        out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
