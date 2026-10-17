pub(crate) mod kitty;

use image::RgbaImage;
use std::io::{self, Write};

pub(crate) struct DisplayOptions {
    pub id: Option<u32>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

pub(crate) trait GraphicsBackend {
    fn transmit(
        &self,
        out: &mut dyn Write,
        image: &RgbaImage,
        options: &DisplayOptions,
    ) -> io::Result<()>;
    fn delete_all(&self) -> io::Result<()>;
    fn delete_all_to(&self, out: &mut dyn Write) -> io::Result<()>;
    fn clear_placements_to(&self, out: &mut dyn Write) -> io::Result<()>;
    fn place_by_id_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KittyBackend;

impl GraphicsBackend for KittyBackend {
    fn transmit(
        &self,
        out: &mut dyn Write,
        image: &RgbaImage,
        options: &DisplayOptions,
    ) -> io::Result<()> {
        kitty::encode_png_to(out, image, options)
    }

    fn delete_all(&self) -> io::Result<()> {
        kitty::delete_all()
    }

    fn delete_all_to(&self, out: &mut dyn Write) -> io::Result<()> {
        kitty::delete_all_to(out)
    }

    fn clear_placements_to(&self, out: &mut dyn Write) -> io::Result<()> {
        kitty::clear_placements_to(out)
    }

    fn place_by_id_to(&self, out: &mut dyn Write, id: u32) -> io::Result<()> {
        kitty::place_by_id_to(out, id)
    }
}
