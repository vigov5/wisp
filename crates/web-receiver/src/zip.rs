//! Minimal STORED (no-compression) ZIP writer for bundling a received
//! multi-file collection into a single browser download.
//!
//! A browser can't reconstruct a folder tree on disk, and the `<a download>`
//! trick sanitises path separators — so a multi-file / folder transfer would
//! otherwise arrive as N flat downloads with the structure lost. Instead we pack
//! everything into one `.zip` that preserves each file's full path, and hand the
//! user a single download.
//!
//! STORED, not deflated, on purpose: the bytes are already in memory, most real
//! payloads (images/video/archives) don't shrink, and skipping compression keeps
//! the wasm dependency-free and the packing instant. Hand-rolled (no `zip`
//! crate) so it stays wasm-clean and never touches `SystemTime` (unavailable in
//! the browser) — timestamps are pinned to the ZIP epoch (1980-01-01).
//!
//! Limitation: no ZIP64, so a single file or the archive as a whole must stay
//! under 4 GiB. That's well within the tab's RAM ceiling anyway.

/// General-purpose flag bit 11: filenames/comments are UTF-8.
const FLAG_UTF8: u16 = 0x0800;
/// DOS date for 1980-01-01 (year=0, month=1, day=1); time left at 0.
const DOS_DATE_1980: u16 = 0x0021;
const METHOD_STORED: u16 = 0;
const VERSION: u16 = 20; // 2.0 — the floor for STORED with a data descriptor-free entry

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;

/// Build a STORED zip archive from `(path, bytes)` entries. Paths use `/` and
/// are stored verbatim (so `folder/sub/file.txt` round-trips as a tree).
pub fn build_stored_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let table = crc32_table();
    // CRC + local-header offset per entry, filled as we stream the local section.
    let mut meta: Vec<(u32, u32)> = Vec::with_capacity(entries.len());
    let mut out: Vec<u8> = Vec::new();

    for (path, data) in entries {
        let name = normalize_path(path);
        let name_bytes = name.as_bytes();
        let crc = crc32(&table, data);
        let size = data.len() as u32;
        let offset = out.len() as u32;
        meta.push((crc, offset));

        out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&DOS_DATE_1980.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);
    }

    let central_offset = out.len() as u32;
    let mut central: Vec<u8> = Vec::new();
    for (i, (path, data)) in entries.iter().enumerate() {
        let name = normalize_path(path);
        let name_bytes = name.as_bytes();
        let (crc, offset) = meta[i];
        let size = data.len() as u32;

        central.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        central.extend_from_slice(&VERSION.to_le_bytes()); // version made by
        central.extend_from_slice(&VERSION.to_le_bytes()); // version needed
        central.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        central.extend_from_slice(&METHOD_STORED.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&DOS_DATE_1980.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);

    let count = entries.len() as u16;
    out.extend_from_slice(&SIG_EOCD.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
    out.extend_from_slice(&count.to_le_bytes()); // entries on this disk
    out.extend_from_slice(&count.to_le_bytes()); // total entries
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

/// Backslashes → forward slashes, leading slashes trimmed (ZIP paths are
/// relative and `/`-separated).
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_owned()
}

fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0usize;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

fn crc32(table: &[u32; 256], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}
