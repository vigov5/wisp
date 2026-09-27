//! Linux framebuffer output.
//!
//! The TG4040 stock firmware exposes a plain `/dev/fb0` (1024x768, 32bpp,
//! 4096-byte stride) with no DRM and no Wayland, so the whole UI is drawn into
//! an in-process ARGB8888 buffer and blitted here once per frame. That keeps
//! the binary free of SDL2 — which TrimUI has not published an SDK for on this
//! model — and lets it link statically against musl.
//!
//! One device quirk drives the pixel conversion below: the framebuffer's alpha
//! byte is composited by the display hardware, so every pixel we write must
//! carry a fully opaque alpha or the UI shows through to black.

use anyhow::Result;

/// Pixels handed to [`Framebuffer::present`] are `0xAARRGGBB`, host-endian.
pub type Argb = u32;

#[cfg(target_os = "linux")]
pub use self::linux::Framebuffer;

#[cfg(not(target_os = "linux"))]
pub use self::stub::Framebuffer;

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    use anyhow::{Context, Result, bail};

    use super::Argb;

    const FBIOGET_VSCREENINFO: u64 = 0x4600;
    const FBIOGET_FSCREENINFO: u64 = 0x4602;
    /// Best-effort tear avoidance; not all Allwinner fb drivers implement it.
    const FBIO_WAITFORVSYNC: u64 = 0x4620;

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug)]
    struct Bitfield {
        offset: u32,
        length: u32,
        msb_right: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug)]
    struct VarScreeninfo {
        xres: u32,
        yres: u32,
        xres_virtual: u32,
        yres_virtual: u32,
        xoffset: u32,
        yoffset: u32,
        bits_per_pixel: u32,
        grayscale: u32,
        red: Bitfield,
        green: Bitfield,
        blue: Bitfield,
        transp: Bitfield,
        nonstd: u32,
        activate: u32,
        height: u32,
        width: u32,
        accel_flags: u32,
        pixclock: u32,
        left_margin: u32,
        right_margin: u32,
        upper_margin: u32,
        lower_margin: u32,
        hsync_len: u32,
        vsync_len: u32,
        sync: u32,
        vmode: u32,
        rotate: u32,
        colorspace: u32,
        reserved: [u32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FixScreeninfo {
        id: [u8; 16],
        smem_start: usize,
        smem_len: u32,
        kind: u32,
        type_aux: u32,
        visual: u32,
        xpanstep: u16,
        ypanstep: u16,
        ywrapstep: u16,
        line_length: u32,
        mmap_start: usize,
        mmap_len: u32,
        accel: u32,
        capabilities: u16,
        reserved: [u16; 2],
    }

    // The kernel writes these structs through `ioctl`, so their layout is a
    // hard ABI contract rather than an internal detail: a field of the wrong
    // width shifts everything after it and the geometry comes back as
    // plausible-looking garbage instead of failing. Pin the sizes so a later
    // edit cannot break that silently.
    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(std::mem::size_of::<VarScreeninfo>() == 160);
        assert!(std::mem::size_of::<FixScreeninfo>() == 80);
    };

    impl Default for FixScreeninfo {
        fn default() -> Self {
            // `id` is a C char array, so a zeroed struct is the correct empty
            // value; deriving Default is not possible for `[u8; 16]` in older
            // editions and spelling it out keeps the intent obvious.
            unsafe { std::mem::zeroed() }
        }
    }

    pub struct Framebuffer {
        file: std::fs::File,
        map: *mut u8,
        map_len: usize,
        width: u32,
        height: u32,
        line_length: u32,
        bytes_per_pixel: u32,
        /// True when the device layout is exactly `0xAARRGGBB`, which lets the
        /// blit skip per-channel shifting and just set the alpha byte.
        fast_argb: bool,
        red: Bitfield,
        green: Bitfield,
        blue: Bitfield,
        transp: Bitfield,
    }

    // The mapping is owned exclusively by this struct and only touched behind
    // `&mut self`, so it is safe to move the handle between threads.
    unsafe impl Send for Framebuffer {}

    impl Framebuffer {
        pub fn open(path: &str) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .with_context(|| format!("open framebuffer {path}"))?;
            let fd = file.as_raw_fd();

            let mut var = VarScreeninfo::default();
            let mut fix = FixScreeninfo::default();
            unsafe {
                if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var) < 0 {
                    return Err(std::io::Error::last_os_error()).context("FBIOGET_VSCREENINFO");
                }
                if libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix) < 0 {
                    return Err(std::io::Error::last_os_error()).context("FBIOGET_FSCREENINFO");
                }
            }

            if var.bits_per_pixel != 32 {
                bail!(
                    "framebuffer is {} bpp; this build only handles 32 bpp",
                    var.bits_per_pixel
                );
            }
            if var.xres == 0 || var.yres == 0 {
                bail!("framebuffer reports a zero-sized screen");
            }
            // `present` walks each row as `*mut u32`, so every row start has to
            // be 4-byte aligned. The TG4040's 4096-byte stride is, but a driver
            // reporting an odd one would make that cast undefined behaviour.
            if fix.line_length % 4 != 0 {
                bail!(
                    "framebuffer stride {} is not a multiple of 4",
                    fix.line_length
                );
            }

            // `smem_len` can exceed one screen when the driver reserves pages
            // for panning. Map whatever it reports, but never less than the
            // visible screen.
            let visible = (fix.line_length as usize) * (var.yres as usize);
            let map_len = (fix.smem_len as usize).max(visible);

            let map = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    map_len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if map == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error()).context("mmap framebuffer");
            }

            let fast_argb = var.red.offset == 16
                && var.green.offset == 8
                && var.blue.offset == 0
                && var.red.length == 8
                && var.green.length == 8
                && var.blue.length == 8;

            tracing::info!(
                target: "wisp_trimui::fb",
                width = var.xres,
                height = var.yres,
                stride = fix.line_length,
                bpp = var.bits_per_pixel,
                red = var.red.offset,
                green = var.green.offset,
                blue = var.blue.offset,
                transp_offset = var.transp.offset,
                transp_length = var.transp.length,
                fast_argb,
                "framebuffer opened"
            );

            Ok(Self {
                file,
                map: map.cast::<u8>(),
                map_len,
                width: var.xres,
                height: var.yres,
                line_length: fix.line_length,
                bytes_per_pixel: var.bits_per_pixel / 8,
                fast_argb,
                red: var.red,
                green: var.green,
                blue: var.blue,
                transp: var.transp,
            })
        }

        pub fn width(&self) -> u32 {
            self.width
        }

        pub fn height(&self) -> u32 {
            self.height
        }

        /// Encode one authoring-side `0xAARRGGBB` pixel into the device layout,
        /// forcing alpha opaque (see the module comment).
        #[inline]
        fn encode(&self, argb: Argb) -> u32 {
            let r = (argb >> 16) & 0xFF;
            let g = (argb >> 8) & 0xFF;
            let b = argb & 0xFF;
            let mut out =
                (r << self.red.offset) | (g << self.green.offset) | (b << self.blue.offset);
            if self.transp.length > 0 {
                out |= 0xFFu32 << self.transp.offset;
            }
            out
        }

        /// Blit a full `width * height` ARGB frame to the screen.
        pub fn present(&mut self, frame: &[Argb]) -> Result<()> {
            let expected = (self.width as usize) * (self.height as usize);
            if frame.len() != expected {
                bail!("frame has {} pixels, screen wants {expected}", frame.len());
            }
            if self.bytes_per_pixel != 4 {
                bail!("unsupported bytes per pixel: {}", self.bytes_per_pixel);
            }

            let stride = self.line_length as usize;
            let row_bytes = (self.width as usize) * 4;
            for y in 0..self.height as usize {
                let dst_offset = y * stride;
                // Guard against a driver whose reported stride overruns the
                // mapping; better a short frame than a wild write.
                if dst_offset + row_bytes > self.map_len {
                    break;
                }
                let src = &frame[y * self.width as usize..][..self.width as usize];
                let dst = unsafe {
                    std::slice::from_raw_parts_mut(
                        self.map.add(dst_offset).cast::<u32>(),
                        self.width as usize,
                    )
                };
                if self.fast_argb {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = s | 0xFF00_0000;
                    }
                } else {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = self.encode(*s);
                    }
                }
            }

            // Ignored on drivers that do not implement it — this is only a
            // tear-reduction nicety, never a correctness requirement.
            unsafe {
                let mut arg: libc::c_int = 0;
                libc::ioctl(self.file.as_raw_fd(), FBIO_WAITFORVSYNC as _, &mut arg);
            }
            Ok(())
        }
    }

    impl Drop for Framebuffer {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.map.cast::<libc::c_void>(), self.map_len);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod stub {
    use anyhow::{Result, bail};

    use super::Argb;

    /// Keeps the crate compiling on developer machines. Every call fails, so a
    /// non-Linux host can still run the unit tests for layout, QR and text.
    pub struct Framebuffer {
        _private: (),
    }

    impl Framebuffer {
        pub fn open(_path: &str) -> Result<Self> {
            bail!("the framebuffer backend is only available on Linux")
        }

        pub fn width(&self) -> u32 {
            0
        }

        pub fn height(&self) -> u32 {
            0
        }

        pub fn present(&mut self, _frame: &[Argb]) -> Result<()> {
            bail!("the framebuffer backend is only available on Linux")
        }
    }
}

/// Screen geometry the UI lays out against, resolved from the device at
/// startup and defaulted to the TG4040 panel when running off-device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
}

impl Default for Screen {
    fn default() -> Self {
        // TrimUI Brick Pro (TG4040): 3.95" 1024x768 panel, landscape framebuffer.
        Self {
            width: 1024,
            height: 768,
        }
    }
}

impl Screen {
    pub fn pixels(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }
}

/// Opens the framebuffer, falling back to an error the caller can report.
pub fn open_default() -> Result<Framebuffer> {
    let path = std::env::var("WISP_TRIMUI_FB").unwrap_or_else(|_| "/dev/fb0".to_owned());
    Framebuffer::open(&path)
}
