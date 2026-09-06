//! Streaming STORED (no-compression) ZIP writer, used to bundle a received
//! multi-file collection into a single browser download.
//!
//! A browser can't reconstruct a folder tree on disk, and the `<a download>`
//! trick sanitises path separators — so a multi-file / folder transfer would
//! otherwise arrive as N flat downloads with the structure lost. Instead we pack
//! everything into one `.zip` that preserves each file's full path, and hand the
//! user a single download.
//!
//! STORED, not deflated, on purpose: most real payloads (images/video/archives)
//! don't shrink, and skipping compression keeps the wasm dependency-free and the
//! packing free. Hand-rolled (no `zip` crate) so it stays wasm-clean and never
//! touches `SystemTime` (unavailable in the browser) — timestamps are pinned to
//! the ZIP epoch (1980-01-01).
//!
//! Streaming shapes two details:
//!
//! - a local header has to be written before its file's bytes, but a CRC is only
//!   known after them, so entries carry the CRC in a trailing data descriptor
//!   (general-purpose flag bit 3) instead;
//! - the archive is no longer bounded by what fits in the tab, so 4 GiB is
//!   reachable and ZIP64 is not optional. It is used per entry, and for the
//!   archive as a whole, exactly when the 32-bit fields would overflow — so
//!   ordinary transfers still produce a plain, maximally-compatible zip.

/// General-purpose flag bit 11: filenames/comments are UTF-8.
const FLAG_UTF8: u16 = 0x0800;
/// General-purpose flag bit 3: sizes and CRC follow the data, not precede it.
const FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
/// DOS date for 1980-01-01 (year=0, month=1, day=1); time left at 0.
const DOS_DATE_1980: u16 = 0x0021;
const METHOD_STORED: u16 = 0;
/// 2.0 — the floor for a STORED entry.
const VERSION_BASE: u16 = 20;
/// 4.5 — the floor for anything using ZIP64 records.
const VERSION_ZIP64: u16 = 45;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_DESCRIPTOR: u32 = 0x0807_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50;
const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;
/// Extra-field header id for the ZIP64 extended information field.
const EXTRA_ZIP64: u16 = 0x0001;

/// Value a 32-bit field carries when the real one lives in a ZIP64 record.
const OVERFLOW_32: u32 = 0xFFFF_FFFF;
/// Same, for the 16-bit entry counts in the end-of-central-directory record.
const OVERFLOW_16: u16 = 0xFFFF;

/// Builds a zip one entry at a time, emitting the bytes the caller has to
/// forward to the sink.
///
/// The caller drives it: [`begin`](Self::begin) before a file's bytes,
/// [`data`](Self::data) alongside every chunk of them (which the caller writes
/// itself — they pass straight through, uncompressed), [`end`](Self::end) after
/// the last one, and [`finish`](Self::finish) once for the archive.
pub struct ZipStream {
    entries: Vec<Entry>,
    /// Bytes emitted so far, i.e. where the next record starts.
    offset: u64,
    open: Option<Entry>,
    crc_table: [u32; 256],
    /// Size or offset past which 32-bit fields stop fitting. Constant in
    /// practice; the tests lower it so the ZIP64 layout can be exercised
    /// without producing four gigabytes to trip it honestly.
    zip64_at: u64,
}

/// One entry's central-directory facts, accumulated as its data streams past.
struct Entry {
    name: String,
    /// Offset of this entry's local header.
    offset: u64,
    size: u64,
    crc: u32,
    /// Set when this entry's size or offset needs 64-bit fields.
    zip64: bool,
}

impl ZipStream {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            offset: 0,
            open: None,
            crc_table: crc32_table(),
            zip64_at: u64::from(OVERFLOW_32),
        }
    }

    /// Start an entry for `path` of `size` bytes. Returns its local header.
    pub fn begin(&mut self, path: &str, size: u64) -> Vec<u8> {
        let name = normalize_path(path);
        let name_bytes = name.as_bytes().to_vec();
        // The offset matters as much as the size: an entry that starts past
        // 4 GiB can't be pointed at from a 32-bit central-directory field, even
        // if the entry itself is tiny.
        let zip64 = size > self.zip64_at || self.offset > self.zip64_at;
        let offset = self.offset;

        let mut out = Vec::with_capacity(30 + name_bytes.len() + 20);
        out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        out.extend_from_slice(&version_needed(zip64).to_le_bytes());
        out.extend_from_slice(&(FLAG_UTF8 | FLAG_DATA_DESCRIPTOR).to_le_bytes());
        out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&DOS_DATE_1980.to_le_bytes());
        // CRC and sizes are unknown here; the data descriptor carries them. The
        // ZIP64 placeholder is what tells a reader that descriptor's size fields
        // are 8 bytes wide rather than 4.
        let placeholder = if zip64 { OVERFLOW_32 } else { 0 };
        out.extend_from_slice(&0u32.to_le_bytes()); // crc32
        out.extend_from_slice(&placeholder.to_le_bytes()); // compressed size
        out.extend_from_slice(&placeholder.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&(if zip64 { 20u16 } else { 0 }).to_le_bytes());
        out.extend_from_slice(&name_bytes);
        if zip64 {
            // Local ZIP64 extra: both sizes, still unknown, so both zero.
            out.extend_from_slice(&EXTRA_ZIP64.to_le_bytes());
            out.extend_from_slice(&16u16.to_le_bytes());
            out.extend_from_slice(&0u64.to_le_bytes()); // uncompressed
            out.extend_from_slice(&0u64.to_le_bytes()); // compressed
        }

        self.offset += out.len() as u64;
        self.open = Some(Entry {
            name,
            offset,
            size: 0,
            crc: 0xFFFF_FFFF,
            zip64,
        });
        out
    }

    /// Account for `chunk`, which the caller writes to the sink verbatim.
    pub fn data(&mut self, chunk: &[u8]) {
        if let Some(entry) = self.open.as_mut() {
            entry.crc = crc32_update(&self.crc_table, entry.crc, chunk);
            entry.size += chunk.len() as u64;
        }
        self.offset += chunk.len() as u64;
    }

    /// Close the open entry. Returns its data descriptor.
    pub fn end(&mut self) -> Vec<u8> {
        let Some(mut entry) = self.open.take() else {
            return Vec::new();
        };
        entry.crc ^= 0xFFFF_FFFF;
        // An entry whose bytes ran past 4 GiB needs a 64-bit descriptor, even
        // if begin() couldn't tell from the offset alone.
        entry.zip64 |= entry.size > self.zip64_at;

        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&SIG_DESCRIPTOR.to_le_bytes());
        out.extend_from_slice(&entry.crc.to_le_bytes());
        if entry.zip64 {
            out.extend_from_slice(&entry.size.to_le_bytes()); // compressed
            out.extend_from_slice(&entry.size.to_le_bytes()); // uncompressed
        } else {
            out.extend_from_slice(&(entry.size as u32).to_le_bytes());
            out.extend_from_slice(&(entry.size as u32).to_le_bytes());
        }

        self.offset += out.len() as u64;
        self.entries.push(entry);
        out
    }

    /// Emit the central directory and the end-of-archive records, along with
    /// the finished archive's total length — which nothing else knows, since
    /// the caller only ever saw it a piece at a time.
    pub fn finish(mut self) -> (Vec<u8>, u64) {
        // A caller that stopped mid-entry still gets a readable archive.
        let mut out = self.end();
        // Whatever that descriptor was, it belongs to the entry section, so the
        // central directory starts after it and is measured from there.
        let central_offset = self.offset;
        let entry_bytes = out.len() as u64;

        for entry in &self.entries {
            let name_bytes = entry.name.as_bytes();
            out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            out.extend_from_slice(&version_needed(entry.zip64).to_le_bytes()); // made by
            out.extend_from_slice(&version_needed(entry.zip64).to_le_bytes()); // needed
            out.extend_from_slice(&(FLAG_UTF8 | FLAG_DATA_DESCRIPTOR).to_le_bytes());
            out.extend_from_slice(&METHOD_STORED.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&DOS_DATE_1980.to_le_bytes());
            out.extend_from_slice(&entry.crc.to_le_bytes());
            let (size_field, offset_field) = if entry.zip64 {
                (OVERFLOW_32, OVERFLOW_32)
            } else {
                (entry.size as u32, entry.offset as u32)
            };
            out.extend_from_slice(&size_field.to_le_bytes()); // compressed
            out.extend_from_slice(&size_field.to_le_bytes()); // uncompressed
            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(&(if entry.zip64 { 28u16 } else { 0 }).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // comment
            out.extend_from_slice(&0u16.to_le_bytes()); // disk number
            out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            out.extend_from_slice(&offset_field.to_le_bytes());
            out.extend_from_slice(name_bytes);
            if entry.zip64 {
                // Order is fixed by the spec: uncompressed, compressed, offset.
                out.extend_from_slice(&EXTRA_ZIP64.to_le_bytes());
                out.extend_from_slice(&24u16.to_le_bytes());
                out.extend_from_slice(&entry.size.to_le_bytes());
                out.extend_from_slice(&entry.size.to_le_bytes());
                out.extend_from_slice(&entry.offset.to_le_bytes());
            }
        }

        let count = self.entries.len() as u64;
        let central_size = out.len() as u64 - entry_bytes;

        let zip64 = count > u64::from(OVERFLOW_16)
            || central_offset > self.zip64_at
            || central_size > self.zip64_at;
        if zip64 {
            let zip64_eocd_offset = central_offset + central_size;
            out.extend_from_slice(&SIG_ZIP64_EOCD.to_le_bytes());
            // Size of this record from here on: 56 fixed bytes minus the 12
            // already written (signature + this field).
            out.extend_from_slice(&44u64.to_le_bytes());
            out.extend_from_slice(&VERSION_ZIP64.to_le_bytes()); // made by
            out.extend_from_slice(&VERSION_ZIP64.to_le_bytes()); // needed
            out.extend_from_slice(&0u32.to_le_bytes()); // this disk
            out.extend_from_slice(&0u32.to_le_bytes()); // disk with central dir
            out.extend_from_slice(&count.to_le_bytes()); // entries on this disk
            out.extend_from_slice(&count.to_le_bytes()); // total entries
            out.extend_from_slice(&central_size.to_le_bytes());
            out.extend_from_slice(&central_offset.to_le_bytes());

            out.extend_from_slice(&SIG_ZIP64_LOCATOR.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // disk with zip64 eocd
            out.extend_from_slice(&zip64_eocd_offset.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes()); // total disks
        }

        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
        let count_field = if zip64 { OVERFLOW_16 } else { count as u16 };
        out.extend_from_slice(&count_field.to_le_bytes()); // entries on this disk
        out.extend_from_slice(&count_field.to_le_bytes()); // total entries
        let size_field = if zip64 {
            OVERFLOW_32
        } else {
            central_size as u32
        };
        let offset_field = if zip64 {
            OVERFLOW_32
        } else {
            central_offset as u32
        };
        out.extend_from_slice(&size_field.to_le_bytes());
        out.extend_from_slice(&offset_field.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length

        // `central_offset` already counts everything up to the central
        // directory, `entry_bytes` of which is in `out` too.
        let total = central_offset + out.len() as u64 - entry_bytes;
        (out, total)
    }
}

fn version_needed(zip64: bool) -> u16 {
    if zip64 { VERSION_ZIP64 } else { VERSION_BASE }
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

/// Fold `data` into a running CRC. The caller holds it pre-inverted (starting
/// at `0xFFFFFFFF`) and inverts once at the end, so a file can be fed in as
/// many chunks as it arrives in.
fn crc32_update(table: &[u32; 256], mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    impl ZipStream {
        /// A writer that treats anything over `at` as needing 64-bit fields, so
        /// the ZIP64 layout can be tested with bytes instead of gigabytes.
        fn with_zip64_at(at: u64) -> Self {
            Self {
                zip64_at: at,
                ..Self::new()
            }
        }
    }

    /// Drive the writer the way the receiver does: one entry at a time, each
    /// file arriving in several chunks.
    fn pack(entries: &[(&str, &[u8])], zip64_at: u64) -> Vec<u8> {
        let mut zip = ZipStream::with_zip64_at(zip64_at);
        let mut archive = Vec::new();
        for (name, data) in entries {
            archive.extend_from_slice(&zip.begin(name, data.len() as u64));
            for chunk in data.chunks(7) {
                zip.data(chunk);
                archive.extend_from_slice(chunk);
            }
            archive.extend_from_slice(&zip.end());
        }
        let (trailer, total) = zip.finish();
        archive.extend_from_slice(&trailer);
        assert_eq!(
            total,
            archive.len() as u64,
            "finish() misreported the archive length"
        );
        archive
    }

    fn u16_at(archive: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(archive[at..at + 2].try_into().expect("in bounds"))
    }
    fn u32_at(archive: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(archive[at..at + 4].try_into().expect("in bounds"))
    }
    fn u64_at(archive: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(archive[at..at + 8].try_into().expect("in bounds"))
    }

    /// Read an archive back the way an unzipper does: find the end record, walk
    /// the central directory, and follow each entry's offset to its data.
    ///
    /// Deliberately independent of the writer's own bookkeeping — it trusts only
    /// what a reader can see — so a header that disagrees with the bytes around
    /// it shows up here.
    fn unpack(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
        let eocd = (0..=archive.len() - 22)
            .rev()
            .find(|&at| u32_at(archive, at) == SIG_EOCD)
            .expect("no end-of-central-directory record");

        let mut count = u16_at(archive, eocd + 10) as u64;
        let mut central = u32_at(archive, eocd + 16) as u64;
        if count == u64::from(OVERFLOW_16) || central == u64::from(OVERFLOW_32) {
            // The real values live in the ZIP64 record the locator points at.
            let locator = eocd - 20;
            assert_eq!(
                u32_at(archive, locator),
                SIG_ZIP64_LOCATOR,
                "no ZIP64 locator"
            );
            let record = u64_at(archive, locator + 8) as usize;
            assert_eq!(
                u32_at(archive, record),
                SIG_ZIP64_EOCD,
                "locator misses the record"
            );
            count = u64_at(archive, record + 32);
            central = u64_at(archive, record + 48);
        }

        let mut at = central as usize;
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(archive, at), SIG_CENTRAL, "bad central header");
            let name_len = u16_at(archive, at + 28) as usize;
            let extra_len = u16_at(archive, at + 30) as usize;
            let name = String::from_utf8(archive[at + 46..at + 46 + name_len].to_vec())
                .expect("names are written as UTF-8");
            let mut size = u32_at(archive, at + 24) as u64;
            let mut local = u32_at(archive, at + 42) as u64;
            if size == u64::from(OVERFLOW_32) || local == u64::from(OVERFLOW_32) {
                let extra = at + 46 + name_len;
                assert_eq!(
                    u16_at(archive, extra),
                    EXTRA_ZIP64,
                    "expected a ZIP64 extra"
                );
                size = u64_at(archive, extra + 4 + 8); // after the uncompressed field
                local = u64_at(archive, extra + 4 + 16);
            }

            assert_eq!(
                u32_at(archive, local as usize),
                SIG_LOCAL,
                "bad local header"
            );
            let local_name = u16_at(archive, local as usize + 26) as usize;
            let local_extra = u16_at(archive, local as usize + 28) as usize;
            let data = local as usize + 30 + local_name + local_extra;
            out.push((name, archive[data..data + size as usize].to_vec()));

            at += 46 + name_len + extra_len + u16_at(archive, at + 32) as usize;
        }
        out
    }

    const FILES: [(&str, &[u8]); 3] = [
        ("album/one.txt", b"hello streaming world"),
        ("album/sub/two.bin", &[7u8; 5000]),
        // Empty files are the easiest thing to get wrong when sizes only show
        // up in a trailing descriptor.
        ("album/empty.txt", b""),
    ];

    #[test]
    fn a_streamed_archive_reads_back_entry_for_entry() {
        let archive = pack(&FILES, u64::from(OVERFLOW_32));
        let read = unpack(&archive);
        assert_eq!(read.len(), FILES.len());
        for ((name, data), (want_name, want_data)) in read.iter().zip(FILES.iter()) {
            assert_eq!(name, want_name);
            assert_eq!(data, want_data);
        }
    }

    /// The same archive with 64-bit sizes and offsets throughout: the layout
    /// shifts (extra fields, a wider descriptor, an extra end record) and a
    /// reader still has to find everything.
    #[test]
    fn a_zip64_archive_reads_back_entry_for_entry() {
        let archive = pack(&FILES, 8);
        let read = unpack(&archive);
        assert_eq!(read.len(), FILES.len());
        for ((name, data), (want_name, want_data)) in read.iter().zip(FILES.iter()) {
            assert_eq!(name, want_name);
            assert_eq!(data, want_data);
        }
        assert!(
            archive.len() > pack(&FILES, u64::from(OVERFLOW_32)).len(),
            "the ZIP64 records should have made the archive longer"
        );
    }

    /// The CRC is the only field a reader can use to notice the data moved
    /// under it, and it is accumulated across chunk boundaries.
    #[test]
    fn each_entry_carries_the_crc_of_its_own_bytes() {
        let table = crc32_table();
        let archive = pack(&FILES, u64::from(OVERFLOW_32));
        let eocd = (0..=archive.len() - 22)
            .rev()
            .find(|&at| u32_at(&archive, at) == SIG_EOCD)
            .expect("no end-of-central-directory record");
        let mut at = u32_at(&archive, eocd + 16) as usize;
        for (_, data) in FILES.iter() {
            let want = crc32_update(&table, 0xFFFF_FFFF, data) ^ 0xFFFF_FFFF;
            assert_eq!(u32_at(&archive, at + 16), want);
            let name_len = u16_at(&archive, at + 28) as usize;
            let extra_len = u16_at(&archive, at + 30) as usize;
            at += 46 + name_len + extra_len;
        }
    }

    #[test]
    fn paths_are_stored_with_forward_slashes_and_no_leading_root() {
        assert_eq!(normalize_path("/album/one.txt"), "album/one.txt");
        assert_eq!(
            normalize_path(&format!("album{}sub{}two.bin", '\u{5c}', '\u{5c}')),
            "album/sub/two.bin"
        );
    }
}
