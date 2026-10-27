pub(crate) mod kitty;

use image::{RgbImage, RgbaImage};
use std::io::{self, Write};
#[cfg(test)]
use std::sync::{Arc, Mutex, PoisonError};

/// The terminal output for the Kitty sequences of `App`.
///
/// The program writes to stdout. A test app writes to a shared buffer, so that no test deletes
/// the images of the real terminal, also with `cargo test -- --nocapture`.
#[derive(Debug, Clone)]
pub(crate) enum GraphicsOutput {
    Stdout,
    #[cfg(test)]
    Buffer(Arc<Mutex<Vec<u8>>>),
}

impl GraphicsOutput {
    /// An empty buffer for a test app.
    #[cfg(test)]
    pub(crate) fn buffer() -> Self {
        Self::Buffer(Arc::default())
    }

    /// Take the bytes that the buffer holds, so that a test can read the sequences of one step.
    #[cfg(test)]
    pub(crate) fn take_bytes(&self) -> Vec<u8> {
        match self {
            Self::Stdout => Vec::new(),
            Self::Buffer(bytes) => {
                std::mem::take(&mut *bytes.lock().unwrap_or_else(PoisonError::into_inner))
            }
        }
    }
}

impl Write for GraphicsOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout => io::stdout().write(buf),
            #[cfg(test)]
            Self::Buffer(bytes) => {
                bytes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout => io::stdout().flush(),
            #[cfg(test)]
            Self::Buffer(_) => Ok(()),
        }
    }
}

/// The default sends the picture and places it at the cursor, without an id and at its pixel
/// size.
///
/// There is no pixel offset (`X=`, `Y=`): a relaying multiplexer such as herdr turns an offset
/// into a stretched picture, so every picture starts at a cell border.
#[derive(Debug, Default)]
pub(crate) struct DisplayOptions {
    pub id: Option<u32>,
    /// The cell box (`c=`, `r=`) that the terminal scales the picture to. `None` keeps the
    /// pixel size.
    pub cell_box: Option<(u16, u16)>,
    /// Send the picture without a placement (`a=t`). A later `a=p` places it by its id. The
    /// frames of a grid loop go out this way, one time each.
    pub transmit_only: bool,
}

/// The pixels per step of [`Pixels::opaque_rgb`]: 16 KB of RGBA, which stays in the L1 cache
/// between the check and the copy.
const OPACITY_CHUNK: usize = 4096;

/// Raw pixels for one transmission: 3 bytes per pixel (RGB, `f=24`) or 4 (RGBA, `f=32`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pixels<'a> {
    pub width: u32,
    pub height: u32,
    /// True for 4 bytes per pixel.
    pub alpha: bool,
    pub data: &'a [u8],
}

impl Pixels<'_> {
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The same pixels with 3 bytes each, when every pixel is opaque. The terminal then reads
    /// a quarter fewer bytes. `None` for RGB pixels and for an image with a transparent pixel.
    ///
    /// The check and the copy go in chunks of [`OPACITY_CHUNK`] pixels, so the first chunk with
    /// a transparent pixel stops the work: a padded grid picture stops in its first rows.
    pub(crate) fn opaque_rgb(&self) -> Option<Vec<u8>> {
        if !self.alpha {
            return None;
        }
        let (pixels, _) = self.data.as_chunks::<4>();
        let mut rgb = vec![0; pixels.len() * 3];
        for (source, target) in pixels
            .chunks(OPACITY_CHUNK)
            .zip(rgb.chunks_mut(OPACITY_CHUNK * 3))
        {
            if source.iter().fold(u8::MAX, |all, pixel| all & pixel[3]) != u8::MAX {
                return None;
            }
            // The array stores let the compiler turn the loop into vector code: 1 ms for a
            // 2736x2052 frame, four times faster than a `copy_from_slice` of 3 bytes per pixel.
            for (&[red, green, blue, _], pixel) in source.iter().zip(target.as_chunks_mut::<3>().0)
            {
                *pixel = [red, green, blue];
            }
        }
        Some(rgb)
    }
}

impl<'a> From<&'a RgbaImage> for Pixels<'a> {
    fn from(image: &'a RgbaImage) -> Self {
        Self {
            width: image.width(),
            height: image.height(),
            alpha: true,
            data: image.as_raw(),
        }
    }
}

impl<'a> From<&'a RgbImage> for Pixels<'a> {
    fn from(image: &'a RgbImage) -> Self {
        Self {
            width: image.width(),
            height: image.height(),
            alpha: false,
            data: image.as_raw(),
        }
    }
}

/// How the pixels reach the terminal.
///
/// A Retina fullscreen frame is about 17 MB of RGB. As base64 through the pty, that is 22 MB
/// of text that the terminal parses and decodes. A temp file on the same machine carries the
/// pixels with a sequence of about 100 bytes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transfer {
    /// Raw RGB or RGBA as base64 through the pty (`t=d`, `f=24` or `f=32`). Every Kitty
    /// terminal reads it.
    #[default]
    Direct,
    /// A PNG as base64 through the pty (`f=100`): the smallest payload for a connection over SSH.
    Png,
    /// Raw RGB or RGBA in a temp file that the terminal reads and deletes (`t=t`). The terminal
    /// must run on this machine, and `kitty::detect_transfer` must have found its support.
    TempFile,
}

pub(crate) trait GraphicsBackend {
    /// Send `pixels` and show them. An RGBA image without a transparent pixel goes out as RGB.
    fn transmit(
        &self,
        out: &mut dyn Write,
        pixels: Pixels<'_>,
        options: &DisplayOptions,
    ) -> io::Result<()>;
    fn delete_all_to(&self, out: &mut dyn Write) -> io::Result<()>;
    fn clear_placements_to(&self, out: &mut dyn Write) -> io::Result<()>;
    fn place_by_id_to(
        &self,
        out: &mut dyn Write,
        id: u32,
        cell_box: Option<(u16, u16)>,
    ) -> io::Result<()>;
    fn delete_image_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()>;
    fn delete_placements_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KittyBackend {
    transfer: Transfer,
}

impl KittyBackend {
    pub(crate) fn new(transfer: Transfer) -> Self {
        Self { transfer }
    }

    /// The medium of the pixels. The terminal player needs [`Transfer::TempFile`], and the
    /// help popup names it.
    pub(crate) fn transfer(self) -> Transfer {
        self.transfer
    }
}

impl GraphicsBackend for KittyBackend {
    fn transmit(
        &self,
        out: &mut dyn Write,
        pixels: Pixels<'_>,
        options: &DisplayOptions,
    ) -> io::Result<()> {
        kitty::transmit_to(out, pixels, options, self.transfer)
    }

    fn delete_all_to(&self, out: &mut dyn Write) -> io::Result<()> {
        kitty::delete_all_to(out)
    }

    fn clear_placements_to(&self, out: &mut dyn Write) -> io::Result<()> {
        kitty::clear_placements_to(out)
    }

    fn place_by_id_to(
        &self,
        out: &mut dyn Write,
        id: u32,
        cell_box: Option<(u16, u16)>,
    ) -> io::Result<()> {
        kitty::place_by_id_to(out, id, cell_box)
    }

    fn delete_image_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()> {
        kitty::delete_image_to(out, id)
    }

    fn delete_placements_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()> {
        kitty::delete_placements_to(out, id)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayOptions, GraphicsBackend, GraphicsOutput, KittyBackend, OPACITY_CHUNK, Pixels,
        Transfer,
    };
    use image::{Rgba, RgbaImage};
    use std::io::Write;

    #[test]
    fn the_test_buffer_collects_the_bytes_until_a_take() {
        let mut out = GraphicsOutput::buffer();
        let mut copy = out.clone();
        KittyBackend::default().delete_all_to(&mut out).unwrap();
        copy.write_all(b"!").unwrap();
        copy.flush().unwrap();

        assert_eq!(out.take_bytes(), b"\x1b_Ga=d,d=A,q=2\x1b\\!");
        assert!(copy.take_bytes().is_empty(), "the clones share one buffer");
        assert!(GraphicsOutput::Stdout.take_bytes().is_empty());
    }

    #[test]
    fn the_opaque_check_looks_at_every_chunk() {
        let mut image = RgbaImage::from_pixel(OPACITY_CHUNK as u32 + 1, 2, Rgba([1, 2, 3, 255]));
        let rgb = Pixels::from(&image).opaque_rgb().unwrap();
        assert_eq!(rgb.len(), image.as_raw().len() / 4 * 3);
        assert_eq!(rgb[rgb.len() - 3..], [1, 2, 3]);

        image.put_pixel(0, 1, Rgba([1, 2, 3, 0]));
        assert!(
            Pixels::from(&image).opaque_rgb().is_none(),
            "a transparent pixel in the last chunk"
        );
        let rgb = image::RgbImage::new(2, 2);
        assert!(
            Pixels::from(&rgb).opaque_rgb().is_none(),
            "RGB stays as it is"
        );
    }

    #[test]
    fn kitty_backend_delegates_through_trait() {
        let backend = KittyBackend::default();
        assert_eq!(backend.transfer(), Transfer::Direct);
        let image = RgbaImage::new(2, 2);
        let mut out = Vec::new();

        backend
            .transmit(
                &mut out,
                (&image).into(),
                &DisplayOptions {
                    id: Some(7),
                    cell_box: Some((5, 3)),
                    transmit_only: false,
                },
            )
            .unwrap();
        backend.delete_all_to(&mut out).unwrap();
        backend.clear_placements_to(&mut out).unwrap();
        backend.place_by_id_to(&mut out, 9, None).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(",i=7,"));
        assert!(text.contains(",c=5,r=3,"));
        assert!(text.contains("\x1b_Ga=d,d=A,q=2\x1b\\"));
        assert!(text.contains("\x1b_Ga=d,d=a,q=2\x1b\\"));
        assert!(text.contains("\x1b_Ga=p,q=2,C=1,i=9\x1b\\"));
    }

    #[test]
    fn a_backend_keeps_its_transfer() {
        let backend = KittyBackend::new(Transfer::TempFile);
        assert_eq!(backend.transfer(), Transfer::TempFile);
        assert_eq!(KittyBackend::new(Transfer::Png).transfer(), Transfer::Png);
    }
}
