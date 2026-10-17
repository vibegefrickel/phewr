//! A small reader for the HEIF boxes (ISO/IEC 23008-12 on ISOBMFF) that hold the transforms of
//! the image items, in safe Rust.
//!
//! libheif applies the `irot` (rotation) and `imir` (mirror) properties of each item itself, but
//! libheif-rs does not show them. A camera can write a thumbnail without the transform of the
//! primary image. A missing 90 or 270 degree rotation changes the shape, but a missing 180 degree
//! rotation or mirror does not. This reader compares the transforms, so that the grid rejects such
//! a thumbnail.
//!
//! The reader reads only the `meta` box: `pitm` (the primary item), `iref` with its `thmb`
//! references, and `iprp` with `ipco` (the properties) and `ipma` (the property indices of each
//! item). It does not read `iinf`, because libheif already lists the thumbnail items.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// The largest `meta` box that the reader loads. An iPhone photo has about 4 KB.
const MAX_META_BYTES: u64 = 16 * 1024 * 1024;

/// A rotation and mirror as an integer 2x2 matrix. Two transforms with the same matrix give the
/// same picture, also if the file writes them in other steps (180 degrees plus a mirror on one
/// axis is the mirror on the other axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Transform([[i8; 2]; 2]);

impl Transform {
    const IDENTITY: Self = Self([[1, 0], [0, 1]]);

    /// `irot`: `quarter_turns` times 90 degrees anti-clockwise.
    fn rotation(quarter_turns: u8) -> Self {
        match quarter_turns % 4 {
            0 => Self::IDENTITY,
            1 => Self([[0, -1], [1, 0]]),
            2 => Self([[-1, 0], [0, -1]]),
            _ => Self([[0, 1], [-1, 0]]),
        }
    }

    /// `imir`: axis 0 is the vertical axis (left and right change), axis 1 the horizontal axis.
    fn mirror(axis: u8) -> Self {
        if axis & 1 == 0 {
            Self([[-1, 0], [0, 1]])
        } else {
            Self([[1, 0], [0, -1]])
        }
    }

    /// This transform, then `next`.
    fn then(self, next: Self) -> Self {
        let (a, b) = (next.0, self.0);
        let cell = |row: usize, col: usize| a[row][0] * b[0][col] + a[row][1] * b[1][col];
        Self([[cell(0, 0), cell(0, 1)], [cell(1, 0), cell(1, 1)]])
    }
}

/// The items of a HEIF file that the thumbnail check needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HeifItems {
    primary: u32,
    /// `(thumbnail, master)` pairs from the `thmb` references.
    thumbnails: Vec<(u32, u32)>,
    /// The transform of each item with an `irot` or `imir` property.
    transforms: HashMap<u32, Transform>,
}

impl HeifItems {
    /// Read the `meta` box of `path`. `None` for a file without a readable `meta` box.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let meta = read_meta(&mut BufReader::new(File::open(path).ok()?)).ok()??;
        Self::parse_meta(&meta)
    }

    /// True if `thumbnail` is a thumbnail of the primary item and has its rotation and mirror.
    pub(crate) fn thumbnail_matches_primary(&self, thumbnail: u32) -> bool {
        self.thumbnails
            .iter()
            .any(|&(thumb, master)| thumb == thumbnail && master == self.primary)
            && self.transform(thumbnail) == self.transform(self.primary)
    }

    fn transform(&self, item: u32) -> Transform {
        self.transforms
            .get(&item)
            .copied()
            .unwrap_or(Transform::IDENTITY)
    }

    /// Parse the content of the `meta` box (a full box: version and flags come first).
    fn parse_meta(meta: &[u8]) -> Option<Self> {
        let mut items = Self::default();
        let mut primary = None;
        let mut properties = Vec::new();
        let mut associations = Vec::new();
        for (kind, body) in boxes(meta.get(4..)?) {
            match &kind {
                b"pitm" => primary = Some(parse_pitm(body)?),
                b"iref" => items.thumbnails = parse_thumbnail_references(body)?,
                b"iprp" => {
                    for (kind, body) in boxes(body) {
                        match &kind {
                            b"ipco" => properties = boxes(body).collect(),
                            b"ipma" => associations = parse_ipma(body)?,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        items.primary = primary?;
        for (item, indices) in associations {
            let transform = indices
                .into_iter()
                .filter_map(|index| properties.get(usize::from(index).checked_sub(1)?))
                .filter_map(|(kind, body)| match kind {
                    b"irot" => Some(Transform::rotation(*body.first()?)),
                    b"imir" => Some(Transform::mirror(*body.first()?)),
                    _ => None,
                })
                .fold(Transform::IDENTITY, Transform::then);
            if transform != Transform::IDENTITY {
                items.transforms.insert(item, transform);
            }
        }
        Some(items)
    }
}

/// Find the top-level `meta` box and load its content. `Ok(None)` if the file has none.
fn read_meta<R: Read + Seek>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    loop {
        let mut header = [0u8; 8];
        if reader.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let is_meta = &header[4..] == b"meta";
        let body_len = match u32::from_be_bytes([header[0], header[1], header[2], header[3]]) {
            // A size of 0 runs to the end of the file.
            0 if is_meta => {
                let mut body = Vec::new();
                reader
                    .by_ref()
                    .take(MAX_META_BYTES)
                    .read_to_end(&mut body)?;
                return Ok(Some(body));
            }
            0 => return Ok(None),
            1 => {
                let mut large = [0u8; 8];
                reader.read_exact(&mut large)?;
                u64::from_be_bytes(large).checked_sub(16)
            }
            size => u64::from(size).checked_sub(8),
        };
        let Some(body_len) = body_len else {
            return Ok(None);
        };
        if is_meta {
            let Ok(len) = usize::try_from(body_len) else {
                return Ok(None);
            };
            if body_len > MAX_META_BYTES {
                return Ok(None);
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body)?;
            return Ok(Some(body));
        }
        let Ok(skip) = i64::try_from(body_len) else {
            return Ok(None);
        };
        reader.seek(SeekFrom::Current(skip))?;
    }
}

/// The child boxes of `data` as `(type, content)`. A box with a wrong size stops the list.
fn boxes(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut rest = data;
    std::iter::from_fn(move || {
        let mut bytes = Bytes(rest);
        let size = bytes.u32()?;
        let kind: [u8; 4] = bytes.take(4)?.try_into().ok()?;
        let (size, header_len) = match size {
            0 => (rest.len(), 8),
            1 => (usize::try_from(bytes.u64()?).ok()?, 16),
            size => (usize::try_from(size).ok()?, 8),
        };
        let body = rest.get(header_len..size)?;
        rest = rest.get(size..)?;
        Some((kind, body))
    })
}

/// `pitm`: the ID of the primary item.
fn parse_pitm(body: &[u8]) -> Option<u32> {
    let mut bytes = Bytes(body);
    let version = bytes.full_box_version()?;
    bytes.item_id(version > 0)
}

/// `iref`: the `(thumbnail, master)` pairs of its `thmb` references.
fn parse_thumbnail_references(body: &[u8]) -> Option<Vec<(u32, u32)>> {
    let mut bytes = Bytes(body);
    let large_ids = bytes.full_box_version()? > 0;
    let mut pairs = Vec::new();
    for (kind, reference) in boxes(bytes.0) {
        if &kind != b"thmb" {
            continue;
        }
        let mut reference = Bytes(reference);
        let from = reference.item_id(large_ids)?;
        for _ in 0..reference.u16()? {
            pairs.push((from, reference.item_id(large_ids)?));
        }
    }
    Some(pairs)
}

/// `ipma`: the property indices (1-based, in order) of each item.
fn parse_ipma(body: &[u8]) -> Option<Vec<(u32, Vec<u16>)>> {
    let mut bytes = Bytes(body);
    let version = bytes.u8()?;
    let flags = bytes.take(3)?;
    let wide_index = flags.get(2).is_some_and(|flags| flags & 1 == 1);
    let count = bytes.u32()?;
    let mut entries = Vec::new();
    for _ in 0..count {
        let item = bytes.item_id(version >= 1)?;
        let associations = bytes.u8()?;
        let mut indices = Vec::with_capacity(usize::from(associations));
        for _ in 0..associations {
            // The top bit is the `essential` flag.
            let index = if wide_index {
                bytes.u16()? & 0x7fff
            } else {
                u16::from(bytes.u8()? & 0x7f)
            };
            indices.push(index);
        }
        entries.push((item, indices));
    }
    Some(entries)
}

/// A cursor over big-endian box data. Each read returns `None` at the end of the data.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(count)?;
        self.0 = tail;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    /// Read the version and flags of a full box, and return the version.
    fn full_box_version(&mut self) -> Option<u8> {
        let version = self.u8()?;
        self.take(3)?;
        Some(version)
    }

    /// An item ID: 32 bits in the newer box versions, 16 bits in version 0.
    fn item_id(&mut self, large: bool) -> Option<u32> {
        if large {
            self.u32()
        } else {
            self.u16().map(u32::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HeifItems, Transform, read_meta};
    use std::io::Cursor;

    /// A box of `kind` around `body`.
    fn bx(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(body.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(&kind);
        out.extend_from_slice(body);
        out
    }

    /// A full box (version 0, no flags) of `kind`.
    fn full(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
        bx(kind, &[&[0, 0, 0, 0], body].concat())
    }

    /// A `meta` body: primary item 1, thumbnail item 2. Property 1 is `irot` with
    /// `primary_rotation`, property 2 is `ispe`. `thumbnail_properties` are the 1-based property
    /// indices of the thumbnail, `extra` are more properties after the first two.
    fn meta(primary_rotation: u8, extra: &[Vec<u8>], thumbnail_properties: &[u8]) -> Vec<u8> {
        let thmb = bx(*b"thmb", &[0, 2, 0, 1, 0, 1]);
        let mut ipco = [bx(*b"irot", &[primary_rotation]), full(*b"ispe", &[0; 8])].concat();
        for property in extra {
            ipco.extend_from_slice(property);
        }
        let mut ipma = vec![0, 0, 0, 2];
        ipma.extend_from_slice(&[0, 1, 2, 0x81, 2]);
        ipma.extend_from_slice(&[0, 2, u8::try_from(thumbnail_properties.len()).unwrap()]);
        ipma.extend_from_slice(thumbnail_properties);
        let iprp = bx(
            *b"iprp",
            &[bx(*b"ipco", &ipco), full(*b"ipma", &ipma)].concat(),
        );
        [
            vec![0, 0, 0, 0],
            full(*b"hdlr", b"pict"),
            full(*b"pitm", &[0, 1]),
            full(*b"iref", &thmb),
            iprp,
        ]
        .concat()
    }

    #[test]
    fn transforms_compose_like_the_pictures() {
        let half_turn = Transform::rotation(2);
        assert_eq!(
            Transform::rotation(1).then(Transform::rotation(1)),
            half_turn
        );
        assert_eq!(
            half_turn.then(Transform::mirror(0)),
            Transform::mirror(1),
            "180 degrees and a mirror on one axis is the mirror on the other axis"
        );
        assert_eq!(Transform::rotation(4), Transform::IDENTITY);
        assert_ne!(Transform::rotation(1), Transform::rotation(3));
    }

    #[test]
    fn a_thumbnail_needs_the_transform_of_the_primary_item() {
        // The thumbnail has the same `irot` (property 1) as the primary item.
        let same = HeifItems::parse_meta(&meta(2, &[], &[1, 2])).unwrap();
        assert!(same.thumbnail_matches_primary(2));
        // The thumbnail has only `ispe`: the 180 degree turn is missing.
        let missing = HeifItems::parse_meta(&meta(2, &[], &[2])).unwrap();
        assert!(!missing.thumbnail_matches_primary(2));
        // Item 1 is not a thumbnail of the primary item.
        assert!(!same.thumbnail_matches_primary(1));
    }

    #[test]
    fn a_mirror_can_replace_a_half_turn_and_a_mirror() {
        // Primary: no rotation. Thumbnail: 180 degrees (property 3), then a mirror on the
        // vertical axis (property 4), which is a mirror on the horizontal axis (property 5).
        let extra = [bx(*b"irot", &[2]), bx(*b"imir", &[0]), bx(*b"imir", &[1])];
        let items = HeifItems::parse_meta(&meta(0, &extra, &[3, 4])).unwrap();
        let mirrored = HeifItems::parse_meta(&meta(0, &extra, &[5])).unwrap();
        assert!(!items.thumbnail_matches_primary(2));
        assert_eq!(items.transform(2), mirrored.transform(2));
    }

    #[test]
    fn broken_box_data_gives_no_items() {
        let mut data = meta(1, &[], &[1]);
        data.truncate(30);
        assert_eq!(HeifItems::parse_meta(&data), None);
        assert_eq!(HeifItems::parse_meta(&[]), None);
    }

    #[test]
    fn the_meta_box_is_found_after_other_boxes() {
        let body = meta(1, &[], &[1]);
        let file = [
            bx(*b"ftyp", b"heic\0\0\0\0mif1heic"),
            bx(*b"free", &[0; 100]),
            bx(*b"meta", &body),
            bx(*b"mdat", &[0; 10]),
        ]
        .concat();
        assert_eq!(read_meta(&mut Cursor::new(file)).unwrap(), Some(body));
        let without = [bx(*b"ftyp", b"heic"), bx(*b"mdat", &[0; 10])].concat();
        assert_eq!(read_meta(&mut Cursor::new(without)).unwrap(), None);
    }
}
