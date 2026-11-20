//! The large icons of the grid cells. With the terminal foreground known, an icon is a Lucide
//! SVG (`assets/icons/`) that `render` turns into pixels for a Kitty image. Without it, the icon
//! is octant blocks (2x4 dots per cell) that Ghostty draws itself. A cell too small for an icon
//! of two rows gets a one-cell symbol. `stamp_play` draws the play triangle into the pixels of a
//! still: Kitty draws a picture above the text.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::Widget;
use ratatui::widgets::canvas::{Canvas, Painter, Shape};

const ICON_HEIGHT_SHARE: f64 = 0.3;
/// The alpha of a rendered icon: the foreground at this share is as dim as `STYLES.dim`.
const ICON_ALPHA: f32 = 0.45;
/// A smaller cell gets the one-cell symbol.
const MIN_ICON_ROWS: u16 = 2;
/// The dots of one octant cell.
const DOTS: (u16, u16) = (2, 4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Icon {
    /// An image whose thumbnail is not decoded yet.
    Loading,
    /// A video whose thumbnail is not decoded yet.
    Video,
    Folder,
    /// The `..` entry.
    Parent,
}

impl Icon {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [Self::Loading, Self::Video, Self::Folder, Self::Parent];

    /// The Lucide icon (ISC license, `assets/icons/LICENSE`).
    const fn svg(self) -> &'static str {
        match self {
            Self::Loading => include_str!("../assets/icons/image.svg"),
            Self::Video => include_str!("../assets/icons/play.svg"),
            Self::Folder => include_str!("../assets/icons/folder.svg"),
            Self::Parent => include_str!("../assets/icons/folder-up.svg"),
        }
    }

    pub(crate) const fn symbol(self) -> &'static str {
        match self {
            Self::Loading => "\u{25cc}",
            Self::Video => "\u{25b6}",
            Self::Folder => "\u{25b8}",
            Self::Parent => "..",
        }
    }

    /// True when the point `(u, v)` of the square from -1 to 1 belongs to the shape. `v` grows
    /// downward.
    pub(crate) fn covers(self, u: f64, v: f64) -> bool {
        match self {
            Self::Loading => picture_covers(u, v),
            // The tip at the right, the flat side at the left, centered on its area.
            Self::Video => u >= -0.6 && v.abs() <= 0.85 * (0.9 - u) / 1.5,
            Self::Folder => {
                let tab = (-0.95..=-0.1).contains(&u) && (-0.72..=-0.5).contains(&v);
                let body = u.abs() <= 0.95 && (-0.55..=0.72).contains(&v);
                tab || body
            }
            Self::Parent => (u - 0.5).hypot(v) <= 0.35 || (u + 0.5).hypot(v) <= 0.35,
        }
    }
}

/// A landscape photo: a frame, a sun and two mountains.
fn picture_covers(u: f64, v: f64) -> bool {
    let outer = u.abs() <= 0.95 && v.abs() <= 0.75;
    let inner = u.abs() <= 0.78 && v.abs() <= 0.58;
    if outer && !inner {
        return true;
    }
    if !inner {
        return false;
    }
    let sun = (u - 0.42).hypot(v + 0.25) <= 0.17;
    let big_mountain = v >= -0.2 && (u + 0.25).abs() <= (v + 0.2) * 0.75;
    let small_mountain = v >= 0.1 && (u - 0.4).abs() <= (v - 0.1) * 0.9;
    sun || big_mountain || small_mountain
}

/// Draw a white play triangle with a dark edge into the middle of `image`. The loop frames
/// replace the still, and the stamp means "not playing yet".
pub(crate) fn stamp_play(image: &mut image::RgbaImage) {
    const FILL: [u8; 4] = [255, 255, 255, 230];
    const EDGE: [u8; 4] = [0, 0, 0, 140];
    /// The edge is the triangle at this scale, drawn first.
    const EDGE_SCALE: f64 = 1.18;
    let (width, height) = image.dimensions();
    let size = tuilibr::cells::round_to_u32(f64::from(height) * ICON_HEIGHT_SHARE);
    if size < 4 || size > width {
        return;
    }
    let (left, top) = ((width - size) / 2, (height - size) / 2);
    for y in 0..size {
        for x in 0..size {
            let u = (f64::from(x) + 0.5) / f64::from(size) * 2.0 - 1.0;
            let v = (f64::from(y) + 0.5) / f64::from(size) * 2.0 - 1.0;
            let color = if Icon::Video.covers(u, v) {
                FILL
            } else if Icon::Video.covers(u / EDGE_SCALE, v / EDGE_SCALE) {
                EDGE
            } else {
                continue;
            };
            let pixel = image.get_pixel_mut(left + x, top + y);
            *pixel = image::Rgba(blend(pixel.0, color));
        }
    }
}

/// Render `icon` into a picture of `size` pixels in `color`, at `ICON_ALPHA`. The icon is a
/// square of the full height, centered. `None` for an empty size.
pub(crate) fn render(icon: Icon, size: (u32, u32), color: [u8; 3]) -> Option<image::RgbaImage> {
    use resvg::{tiny_skia, usvg};

    let tree = usvg::Tree::from_str(icon.svg(), &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size.0, size.1)?;
    let side = size.0.min(size.1) as f32;
    let scale = side / tree.size().width().max(tree.size().height());
    let left = (size.0 as f32 - side) / 2.0;
    let top = (size.1 as f32 - side) / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(left, top);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // The SVG strokes in black, so only the alpha carries the shape.
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the alpha byte times a share of at most 1 stays in 0..=255"
            )]
            let alpha = (f32::from(pixel.alpha()) * ICON_ALPHA).round() as u8;
            [color[0], color[1], color[2], alpha]
        })
        .collect();
    image::RgbaImage::from_raw(size.0, size.1, pixels)
}

/// `over` on `under`, with the alpha of `over`.
fn blend(under: [u8; 4], over: [u8; 4]) -> [u8; 4] {
    let alpha = u32::from(over[3]);
    // A weighted mean of two bytes fits into a byte.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a weighted mean of two bytes is at most 255"
    )]
    let mix = |u: u8, o: u8| ((u32::from(o) * alpha + u32::from(u) * (255 - alpha)) / 255) as u8;
    [
        mix(under[0], over[0]),
        mix(under[1], over[1]),
        mix(under[2], over[2]),
        under[3].max(over[3]),
    ]
}

/// The icon rows for a box of `thumb_rows` rows. `None` below two rows. The count never falls
/// when the box grows, so no smaller box holds an icon if this box holds none.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a rounded share of a u16 row count stays in range"
)]
pub(crate) fn icon_rows(thumb_rows: u16) -> Option<u16> {
    let rows = (f64::from(thumb_rows) * ICON_HEIGHT_SHARE).round() as u16;
    (rows >= MIN_ICON_ROWS).then_some(rows)
}

/// The icon size in cells (columns, rows) for a box of `thumb_box` cells: a square in pixels,
/// 30 % of the box height. `None` when it does not fit or has fewer than two rows.
pub(crate) fn icon_size(thumb_box: (u16, u16), cell_px: (u32, u32)) -> Option<(u16, u16)> {
    let rows = icon_rows(thumb_box.1)?;
    if cell_px.0 == 0 {
        return None;
    }
    let height_px = u32::from(rows) * cell_px.1;
    let cols = u16::try_from(height_px.div_ceil(cell_px.0)).ok()?;
    (cols <= thumb_box.0).then_some((cols, rows))
}

/// The icon box in `thumb`, centered. `None` when `icon_size` gives none.
pub(crate) fn icon_rect(thumb: Rect, cell_px: (u32, u32)) -> Option<Rect> {
    let (cols, rows) = icon_size((thumb.width, thumb.height), cell_px)?;
    Some(tuilibr::pictures::kitty::placement::centered(
        thumb, cols, rows,
    ))
}

/// Draw `icon` into `area` in the foreground color of `style`.
pub(crate) fn draw_icon(buf: &mut Buffer, area: Rect, icon: Icon, style: Style) {
    let color = style.fg.unwrap_or(Color::Reset);
    let shape = IconShape {
        icon,
        dots: (area.width * DOTS.0, area.height * DOTS.1),
        color,
    };
    Canvas::default()
        .marker(Marker::Octant)
        .x_bounds([0.0, f64::from(shape.dots.0)])
        .y_bounds([0.0, f64::from(shape.dots.1)])
        .paint(|ctx| ctx.draw(&shape))
        .render(area, buf);
}

struct IconShape {
    icon: Icon,
    dots: (u16, u16),
    color: Color,
}

impl Shape for IconShape {
    fn draw(&self, painter: &mut Painter<'_, '_>) {
        let (width, height) = (f64::from(self.dots.0), f64::from(self.dots.1));
        for y in 0..self.dots.1 {
            for x in 0..self.dots.0 {
                // The center of the dot, from -1 to 1 on both axes.
                let u = (f64::from(x) + 0.5) / width * 2.0 - 1.0;
                let v = (f64::from(y) + 0.5) / height * 2.0 - 1.0;
                if self.icon.covers(u, v) {
                    painter.paint(usize::from(x), usize::from(y), self.color);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ICON_ALPHA, Icon, draw_icon, icon_rect, render, stamp_play};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};

    /// Ghostty's cell size on the maintainer's machine.
    const CELL_PX: (u32, u32) = (18, 43);

    #[test]
    fn icon_box_is_square_and_centered() {
        let cell = Rect::new(10, 5, 30, 10);
        let icon = icon_rect(cell, CELL_PX).unwrap();
        assert_eq!(icon.height, 3);
        // 3 rows of 43 px are 129 px, so 8 columns of 18 px.
        assert_eq!(icon.width, 8);
        assert_eq!((icon.x, icon.y), (21, 8));
    }

    #[test]
    fn small_cell_has_no_icon_box() {
        assert_eq!(icon_rect(Rect::new(0, 0, 30, 4), CELL_PX), None);
        assert_eq!(icon_rect(Rect::new(0, 0, 4, 10), CELL_PX), None);
        assert_eq!(icon_rect(Rect::new(0, 0, 30, 10), (0, 0)), None);
    }

    #[test]
    fn icons_draw_dots_in_style_color() {
        for icon in [Icon::Loading, Icon::Video, Icon::Folder, Icon::Parent] {
            let area = Rect::new(0, 0, 10, 4);
            let mut buf = Buffer::empty(area);
            draw_icon(&mut buf, area, icon, Style::new().fg(Color::DarkGray));
            let drawn: Vec<_> = buf
                .content()
                .iter()
                .filter(|cell| cell.symbol() != " ")
                .collect();
            assert!(drawn.len() >= 8, "{icon:?} draws a visible shape");
            assert!(drawn.iter().all(|cell| cell.fg == Color::DarkGray));
        }
    }

    #[test]
    fn svg_icons_render_in_color() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "255 times a share of at most 1 stays in 0..=255"
        )]
        let max_alpha = (255.0 * ICON_ALPHA).round() as u8;
        for icon in Icon::ALL {
            let picture = render(icon, (72, 43), [10, 200, 30]).unwrap();
            assert_eq!(picture.dimensions(), (72, 43));
            let drawn: Vec<_> = picture.pixels().filter(|pixel| pixel[3] > 0).collect();
            assert!(drawn.len() > 100, "{icon:?} draws a visible shape");
            assert!(drawn.iter().all(|pixel| pixel.0[..3] == [10, 200, 30]));
            assert!(drawn.iter().all(|pixel| pixel[3] <= max_alpha));
            assert_eq!(
                picture.get_pixel(0, 0)[3],
                0,
                "{icon:?}: the corner is clear"
            );
            assert_eq!(
                picture.get_pixel(71, 21)[3],
                0,
                "{icon:?}: the square is centered"
            );
        }
        assert!(render(Icon::Folder, (0, 10), [0, 0, 0]).is_none());
    }

    #[test]
    fn shapes_have_their_parts() {
        assert!(Icon::Loading.covers(0.9, 0.0), "the frame");
        assert!(Icon::Loading.covers(0.42, -0.25), "the sun");
        assert!(Icon::Loading.covers(-0.25, 0.3), "the mountain");
        assert!(!Icon::Loading.covers(-0.6, -0.4), "the sky");
        assert!(Icon::Video.covers(-0.5, 0.0));
        assert!(Icon::Video.covers(0.8, 0.0));
        assert!(!Icon::Video.covers(0.8, 0.5));
        assert!(Icon::Parent.covers(-0.5, 0.0) && Icon::Parent.covers(0.5, 0.0));
        assert!(!Icon::Parent.covers(0.0, 0.0), "two dots, not one bar");
    }

    #[test]
    fn stamp_draws_edged_white_triangle() {
        let mut image = image::RgbaImage::from_pixel(200, 100, image::Rgba([40, 40, 40, 255]));
        stamp_play(&mut image);
        let center = image.get_pixel(100, 50).0;
        assert!(
            center[0] > 200 && center[3] == 255,
            "white fill: {center:?}"
        );
        assert_eq!(
            image.get_pixel(2, 2).0,
            [40, 40, 40, 255],
            "the corner stays"
        );
        // Left of the flat side of the triangle sits the dark edge.
        let box_left = (200 - 30) / 2;
        let edge = image.get_pixel(box_left + 4, 50).0;
        assert!(edge[0] < 40, "dark edge: {edge:?}");

        let mut tiny = image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 40, 40, 255]));
        stamp_play(&mut tiny);
        assert_eq!(
            tiny.get_pixel(4, 4).0,
            [40, 40, 40, 255],
            "too small for a stamp"
        );
    }
}
