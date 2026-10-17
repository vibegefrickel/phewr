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

#[cfg(test)]
mod tests {
    use super::{DisplayOptions, GraphicsBackend, KittyBackend};
    use image::RgbaImage;

    #[test]
    fn kitty_backend_delegates_through_trait() {
        let backend = KittyBackend;
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
                },
            )
            .unwrap();
        backend.delete_all_to(&mut out).unwrap();
        backend.clear_placements_to(&mut out).unwrap();
        backend.place_by_id_to(&mut out, 9).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(",i=7,"));
        assert!(text.contains(",c=5,r=3,"));
        assert!(text.contains("\x1b_Ga=d,d=A,q=2\x1b\\"));
        assert!(text.contains("\x1b_Ga=d,d=a,q=2\x1b\\"));
        assert!(text.contains("\x1b_Ga=p,q=2,C=1,i=9\x1b\\"));
    }
}
