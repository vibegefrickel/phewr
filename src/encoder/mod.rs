pub(crate) mod kitty;

use image::RgbaImage;
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

pub(crate) struct DisplayOptions {
    pub id: Option<u32>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// The pixel offset inside the first cell (`X=` and `Y=`), so that a picture can start
    /// between two cell borders.
    pub offset: (u32, u32),
}

/// How the pixels reach the terminal.
///
/// A Retina fullscreen frame is about 22 MB of RGBA. As base64 through the pty, that is 29 MB
/// of text that the terminal parses and decodes. A temp file on the same machine carries the
/// pixels with a sequence of about 100 bytes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transfer {
    /// Raw RGBA as base64 through the pty (`t=d`, `f=32`). Every Kitty terminal reads it.
    #[default]
    Direct,
    /// A PNG as base64 through the pty (`f=100`): the smallest payload for a connection over SSH.
    Png,
    /// Raw RGBA in a temp file that the terminal reads and deletes (`t=t`). The terminal must
    /// run on this machine, and `kitty::detect_transfer` must have found its support.
    TempFile,
}

pub(crate) trait GraphicsBackend {
    fn transmit(
        &self,
        out: &mut dyn Write,
        image: &RgbaImage,
        options: &DisplayOptions,
    ) -> io::Result<()>;
    fn delete_all_to(&self, out: &mut dyn Write) -> io::Result<()>;
    fn clear_placements_to(&self, out: &mut dyn Write) -> io::Result<()>;
    fn place_by_id_to(&self, out: &mut dyn Write, id: u32, offset: (u32, u32)) -> io::Result<()>;
    fn delete_image_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KittyBackend {
    transfer: Transfer,
}

impl KittyBackend {
    pub(crate) fn new(transfer: Transfer) -> Self {
        Self { transfer }
    }

    #[cfg(test)]
    pub(crate) fn transfer(self) -> Transfer {
        self.transfer
    }
}

impl GraphicsBackend for KittyBackend {
    fn transmit(
        &self,
        out: &mut dyn Write,
        image: &RgbaImage,
        options: &DisplayOptions,
    ) -> io::Result<()> {
        kitty::transmit_to(out, image, options, self.transfer)
    }

    fn delete_all_to(&self, out: &mut dyn Write) -> io::Result<()> {
        kitty::delete_all_to(out)
    }

    fn clear_placements_to(&self, out: &mut dyn Write) -> io::Result<()> {
        kitty::clear_placements_to(out)
    }

    fn place_by_id_to(&self, out: &mut dyn Write, id: u32, offset: (u32, u32)) -> io::Result<()> {
        kitty::place_by_id_to(out, id, offset)
    }

    fn delete_image_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()> {
        kitty::delete_image_to(out, id)
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayOptions, GraphicsBackend, GraphicsOutput, KittyBackend, Transfer};
    use image::RgbaImage;
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
    fn kitty_backend_delegates_through_trait() {
        let backend = KittyBackend::default();
        assert_eq!(backend.transfer(), Transfer::Direct);
        let image = RgbaImage::new(2, 2);
        let mut out = Vec::new();

        backend
            .transmit(
                &mut out,
                &image,
                &DisplayOptions {
                    id: Some(7),
                    cols: Some(5),
                    rows: Some(3),
                    offset: (0, 0),
                },
            )
            .unwrap();
        backend.delete_all_to(&mut out).unwrap();
        backend.clear_placements_to(&mut out).unwrap();
        backend.place_by_id_to(&mut out, 9, (0, 0)).unwrap();

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
