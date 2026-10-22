//! The JPEG previews inside camera RAW files and Photoshop files, in safe Rust.
//!
//! A camera writes a JPEG of the photo into its RAW file, often at full size. Most RAW formats
//! are TIFF: an IFD holds the JPEG as `JPEGInterchangeFormat` and `JPEGInterchangeFormatLength`
//! (tags 0x201 and 0x202), as the one strip of a JPEG-compressed image (`StripOffsets`,
//! `StripByteCounts`, `Compression` 6 or 7), or as the Panasonic `JpgFromRaw` tag (0x2e). The
//! reader walks IFD0, its `SubIFDs` (0x14a) and the IFD chain, and takes the largest JPEG. The
//! raw sensor data of a DNG or a CR2 is a lossless JPEG (SOF3), which the SOF check rejects.
//! Canon CR3 is ISOBMFF: the `PRVW` and `THMB` boxes hold the JPEGs, `CMT1` holds the TIFF with
//! the orientation. Fujifilm RAF has a fixed header with the JPEG offset and length. A Photoshop
//! file carries its thumbnail in the image resource 1036 (JPEG RGB).
//!
//! sips takes 0.1 s to 1 s per RAW file. The embedded JPEG decodes in a few milliseconds, so the
//! grid shows it at once. When it covers the box, it is the sharp thumbnail and sips never runs.

use crate::decode::raster::decode_jpeg_bytes;
use crate::info;
use crate::scanner;
use image::DynamicImage;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// The `ImageIO` formats of the sips route without an embedded JPEG.
const WITHOUT_PREVIEW: &[&str] = &["jp2", "pdf", "svg", "icns", "sgi", "dds", "pict"];

/// The largest JPEG that the reader loads. A full-size preview of a 45 MP camera is 20 MB.
const MAX_JPEG_BYTES: u64 = 128 * 1024 * 1024;

/// The bytes of a JPEG that the reader loads for the SOF scan. The APP segments (EXIF, XMP,
/// ICC) come first, each at most 64 KB.
const SOF_SCAN_BYTES: u64 = 512 * 1024;

/// The largest `moov` or `uuid` box of a CR3 that the reader loads. The `PRVW` box holds a
/// 1620x1080 JPEG, about 1 MB.
const MAX_CR3_BOX_BYTES: u64 = 64 * 1024 * 1024;

/// The largest image resource section of a PSD that the reader loads.
const MAX_PSD_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// The IFDs that the TIFF walk visits, at most. A RAW file has 2 to 6.
const MAX_IFDS: usize = 32;

/// The entries of one IFD, at most. A maker note IFD has a few hundred.
const MAX_IFD_ENTRIES: u16 = 1024;

/// An embedded JPEG: where it sits in the file, the size as the file stores it, and the EXIF
/// orientation of the file around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmbeddedJpeg {
    pub(crate) offset: u64,
    pub(crate) length: u64,
    /// The stored size, before the orientation.
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The EXIF orientation (1 to 8) that the decode applies: the one of the container (IFD0
    /// of a TIFF RAW, `CMT1` of a CR3), else the one in the EXIF of the JPEG. exiftool and the
    /// RAW developers read the container first, too.
    pub(crate) orientation: Option<u8>,
}

impl EmbeddedJpeg {
    /// The size after the orientation.
    pub(crate) fn shown_size(&self) -> (u32, u32) {
        if self.orientation.is_some_and(|o| (5..=8).contains(&o)) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// True when the picture is at least as large as `box_px` on both sides, so that it fits
    /// the box without an upscale.
    pub(crate) fn covers(&self, box_px: (u32, u32)) -> bool {
        let (width, height) = self.shown_size();
        width >= box_px.0 && height >= box_px.1
    }

    fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// True for a file of the sips route that can carry an embedded JPEG: camera RAW and PSD.
pub(crate) fn may_have_embedded_jpeg(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let listed = |list: &[&str]| list.iter().any(|e| ext.eq_ignore_ascii_case(e));
    listed(scanner::SIPS_EXTENSIONS) && !listed(WITHOUT_PREVIEW)
}

/// The largest embedded JPEG of `path`, or `None` when the file has none that the reader finds.
pub(crate) fn find(path: &Path) -> Option<EmbeddedJpeg> {
    if !may_have_embedded_jpeg(path) {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mut source = Source::open(path).ok()?;
    match ext.as_str() {
        "psd" => find_in_psd(&mut source),
        "cr3" => find_in_cr3(&mut source),
        "raf" => find_in_raf(&mut source),
        _ => find_in_tiff(&mut source, 0),
    }
}

/// Decode the embedded JPEG `jpeg` of `path`, scaled down to `target` when there is one.
pub(crate) fn decode(
    path: &Path,
    jpeg: &EmbeddedJpeg,
    target: Option<(u32, u32)>,
) -> io::Result<DynamicImage> {
    let mut source = Source::open(path)?;
    let data = source.read_at(jpeg.offset, jpeg.length)?;
    decode_jpeg_bytes(&data, target, jpeg.orientation)
}

/// The embedded JPEG of `path` when it covers `box_px`, decoded for that box. `None` when the
/// file has none or the JPEG is too small, so that the caller decodes the primary image.
pub(crate) fn decode_covering(path: &Path, box_px: (u32, u32)) -> Option<DynamicImage> {
    let jpeg = find(path).filter(|jpeg| jpeg.covers(box_px))?;
    decode(path, &jpeg, Some(box_px)).ok()
}

/// A file with its length, for bounded reads at an offset.
struct Source<R> {
    reader: R,
    len: u64,
}

impl Source<File> {
    fn open(path: &Path) -> io::Result<Self> {
        let reader = File::open(path)?;
        let len = reader.metadata()?.len();
        Ok(Self { reader, len })
    }
}

impl<R: Read + Seek> Source<R> {
    fn from_reader(mut reader: R) -> io::Result<Self> {
        let len = reader.seek(SeekFrom::End(0))?;
        Ok(Self { reader, len })
    }

    /// True when `offset` plus `length` lies inside the file.
    fn holds(&self, offset: u64, length: u64) -> bool {
        offset
            .checked_add(length)
            .is_some_and(|end| end <= self.len)
    }

    /// `length` bytes from `offset`. A range outside the file is an error.
    fn read_at(&mut self, offset: u64, length: u64) -> io::Result<Vec<u8>> {
        if !self.holds(offset, length) || length > MAX_JPEG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the embedded data lies outside the file",
            ));
        }
        let count =
            usize::try_from(length).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.reader.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; count];
        self.reader.read_exact(&mut data)?;
        Ok(data)
    }

    /// The JPEG at `offset` with `length` bytes, when the data starts with a JPEG that a
    /// baseline or progressive decoder reads. The size comes from its SOF segment.
    /// The orientation is the one of the EXIF in the JPEG. The caller puts the container
    /// orientation over it.
    fn jpeg_at(&mut self, offset: u64, length: u64) -> Option<EmbeddedJpeg> {
        if length < 4 || !self.holds(offset, length) {
            return None;
        }
        let head = self.read_at(offset, length.min(SOF_SCAN_BYTES)).ok()?;
        let (width, height) = jpeg_size(&head)?;
        Some(EmbeddedJpeg {
            offset,
            length,
            width,
            height,
            orientation: info::orientation_from_bytes(&head),
        })
    }
}

/// The size from the SOF segment of the JPEG `data`. `None` for data that is no JPEG, for a
/// lossless JPEG (SOF3, the raw sensor data of a DNG or CR2) and for an arithmetic one.
pub(crate) fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 || data[0..3] != [0xFF, 0xD8, 0xFF] {
        return None;
    }
    let mut pos = 2;
    loop {
        if *data.get(pos)? != 0xFF {
            return None;
        }
        let marker = *data.get(pos + 1)?;
        match marker {
            // A fill byte, or a marker without a length: the start of the image, a restart.
            0xFF => pos += 1,
            0x01 | 0xD0..=0xD8 => pos += 2,
            0xC0..=0xC2 => {
                let height = u32::from(u16::from_be_bytes([
                    *data.get(pos + 5)?,
                    *data.get(pos + 6)?,
                ]));
                let width = u32::from(u16::from_be_bytes([
                    *data.get(pos + 7)?,
                    *data.get(pos + 8)?,
                ]));
                return (width > 0 && height > 0).then_some((width, height));
            }
            // The other SOF markers (lossless, hierarchical, arithmetic) and the scan data.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDA => return None,
            _ => {
                let length = usize::from(u16::from_be_bytes([
                    *data.get(pos + 2)?,
                    *data.get(pos + 3)?,
                ]));
                pos += 2 + length;
            }
        }
    }
}

/// The byte order of a TIFF header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn u16(self, bytes: &[u8]) -> Option<u16> {
        let pair = [*bytes.first()?, *bytes.get(1)?];
        Some(match self {
            Self::Little => u16::from_le_bytes(pair),
            Self::Big => u16::from_be_bytes(pair),
        })
    }

    fn u32(self, bytes: &[u8]) -> Option<u32> {
        let quad = [
            *bytes.first()?,
            *bytes.get(1)?,
            *bytes.get(2)?,
            *bytes.get(3)?,
        ];
        Some(match self {
            Self::Little => u32::from_le_bytes(quad),
            Self::Big => u32::from_be_bytes(quad),
        })
    }
}

/// One IFD entry: the tag, the field type, the count and the 4 value bytes.
#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value: [u8; 4],
}

impl Entry {
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    const IFD: u16 = 13;

    /// The one value of a SHORT or LONG entry with count 1.
    fn scalar(&self, order: ByteOrder) -> Option<u32> {
        if self.count != 1 {
            return None;
        }
        match self.kind {
            Self::SHORT => order.u16(&self.value).map(u32::from),
            Self::LONG | Self::IFD => order.u32(&self.value),
            _ => None,
        }
    }

    /// The offset of the data of an entry that does not fit the 4 value bytes.
    fn data_offset(&self, order: ByteOrder) -> Option<u32> {
        order.u32(&self.value)
    }
}

/// The entries of one IFD and the offset of the next IFD in the chain.
struct Ifd {
    entries: Vec<Entry>,
    next: u32,
}

impl Ifd {
    fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }

    fn scalar(&self, tag: u16, order: ByteOrder) -> Option<u32> {
        self.get(tag)?.scalar(order)
    }
}

/// The TIFF header at `base`: the byte order. The magic is 42 for TIFF and DNG, `RO` and `RS`
/// for Olympus ORF and 85 for Panasonic RW2.
fn tiff_order<R: Read + Seek>(source: &mut Source<R>, base: u64) -> Option<(ByteOrder, u32)> {
    let header = source.read_at(base, 8).ok()?;
    let order = match &header[0..2] {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return None,
    };
    let magic = order.u16(&header[2..4])?;
    if !matches!(magic, 42 | 0x4F52 | 0x5352 | 0x55) {
        return None;
    }
    Some((order, order.u32(&header[4..8])?))
}

/// The IFD at `base + offset`.
fn read_ifd<R: Read + Seek>(
    source: &mut Source<R>,
    base: u64,
    offset: u32,
    order: ByteOrder,
) -> Option<Ifd> {
    let start = base.checked_add(u64::from(offset))?;
    let count = order.u16(&source.read_at(start, 2).ok()?)?;
    if count == 0 || count > MAX_IFD_ENTRIES {
        return None;
    }
    let body = source.read_at(start + 2, u64::from(count) * 12 + 4).ok()?;
    let entries = body
        .as_chunks::<12>()
        .0
        .iter()
        .take(usize::from(count))
        .filter_map(|chunk| {
            Some(Entry {
                tag: order.u16(&chunk[0..2])?,
                kind: order.u16(&chunk[2..4])?,
                count: order.u32(&chunk[4..8])?,
                value: [chunk[8], chunk[9], chunk[10], chunk[11]],
            })
        })
        .collect();
    let next = order.u32(&body[usize::from(count) * 12..])?;
    Some(Ifd { entries, next })
}

/// The `SubIFDs` offsets of an IFD (tag 0x14a): inline for one, else in the data.
fn sub_ifd_offsets<R: Read + Seek>(
    source: &mut Source<R>,
    base: u64,
    ifd: &Ifd,
    order: ByteOrder,
) -> Vec<u32> {
    let Some(entry) = ifd.get(0x14A) else {
        return Vec::new();
    };
    if !matches!(entry.kind, Entry::LONG | Entry::IFD) {
        return Vec::new();
    }
    if entry.count == 1 {
        return entry.scalar(order).into_iter().collect();
    }
    let count = entry.count.min(16);
    let Some(offset) = entry.data_offset(order) else {
        return Vec::new();
    };
    let Ok(data) = source.read_at(base + u64::from(offset), u64::from(count) * 4) else {
        return Vec::new();
    };
    data.as_chunks::<4>()
        .0
        .iter()
        .filter_map(|c| order.u32(c))
        .collect()
}

/// The JPEG candidates of one IFD: the `JPEGInterchangeFormat` pair, the one strip of a
/// JPEG-compressed image, and the Panasonic `JpgFromRaw` blob.
fn ifd_jpeg_ranges(ifd: &Ifd, order: ByteOrder) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    if let (Some(offset), Some(length)) = (ifd.scalar(0x201, order), ifd.scalar(0x202, order)) {
        ranges.push((offset, length));
    }
    let compression = ifd.scalar(0x103, order);
    if matches!(compression, Some(6 | 7))
        && let (Some(offset), Some(length)) = (ifd.scalar(0x111, order), ifd.scalar(0x117, order))
    {
        ranges.push((offset, length));
    }
    if let Some(entry) = ifd.get(0x2E)
        && entry.count > 4
        && let Some(offset) = entry.data_offset(order)
    {
        ranges.push((offset, entry.count));
    }
    ranges
}

/// The largest JPEG of the TIFF at `base`: IFD0, its `SubIFDs` and the IFD chain. The
/// orientation comes from IFD0.
fn find_in_tiff<R: Read + Seek>(source: &mut Source<R>, base: u64) -> Option<EmbeddedJpeg> {
    let (order, first) = tiff_order(source, base)?;
    let mut queue = vec![first];
    let mut seen = HashSet::new();
    let mut orientation = None;
    let mut best: Option<EmbeddedJpeg> = None;
    while let Some(offset) = queue.pop() {
        if offset == 0 || !seen.insert(offset) || seen.len() > MAX_IFDS {
            continue;
        }
        let Some(ifd) = read_ifd(source, base, offset, order) else {
            continue;
        };
        if offset == first {
            orientation = ifd
                .scalar(0x112, order)
                .and_then(|o| u8::try_from(o).ok())
                .filter(|o| (1..=8).contains(o));
        }
        for (start, length) in ifd_jpeg_ranges(&ifd, order) {
            let found = source.jpeg_at(base + u64::from(start), u64::from(length));
            if let Some(found) = found
                && best.is_none_or(|best| found.pixels() > best.pixels())
            {
                best = Some(found);
            }
        }
        queue.extend(sub_ifd_offsets(source, base, &ifd, order));
        queue.push(ifd.next);
    }
    best.map(|jpeg| EmbeddedJpeg {
        orientation: orientation.or(jpeg.orientation),
        ..jpeg
    })
}

/// The JPEG of a Fujifilm RAF: the header names its offset and length at byte 84. The JPEG has
/// its own EXIF orientation.
fn find_in_raf<R: Read + Seek>(source: &mut Source<R>) -> Option<EmbeddedJpeg> {
    let header = source.read_at(0, 92).ok()?;
    if !header.starts_with(b"FUJIFILM") {
        return None;
    }
    let offset = ByteOrder::Big.u32(&header[84..88])?;
    let length = ByteOrder::Big.u32(&header[88..92])?;
    source.jpeg_at(u64::from(offset), u64::from(length))
}

/// The largest JPEG of a Canon CR3: the `PRVW` box (1620x1080) or the `THMB` box (160x120). The
/// orientation sits in the TIFF of the `CMT1` box. The reader loads only the `moov` and `uuid`
/// boxes, never the `mdat` box with the sensor data.
fn find_in_cr3<R: Read + Seek>(source: &mut Source<R>) -> Option<EmbeddedJpeg> {
    let mut offset = 0;
    let mut best: Option<EmbeddedJpeg> = None;
    let mut orientation = None;
    // A bad box ends the walk, but keeps the JPEG and the orientation found before it.
    while let Some((size, head, tag)) = cr3_box_at(source, offset) {
        if matches!(&tag, b"moov" | b"uuid")
            && size <= MAX_CR3_BOX_BYTES
            && let Ok(body) = source.read_at(offset + head, size - head)
        {
            let body_offset = offset + head;
            for (tag, start, end) in child_boxes(&body, &[*b"PRVW", *b"THMB", *b"CMT1"]) {
                if tag == *b"CMT1" {
                    orientation = tiff_orientation(&body[start..end]);
                    continue;
                }
                let found = cr3_jpeg_in(source, &body, body_offset, start, end);
                if let Some(found) = found
                    && best.is_none_or(|best| found.pixels() > best.pixels())
                {
                    best = Some(found);
                }
            }
        }
        offset += size;
    }
    best.map(|jpeg| EmbeddedJpeg {
        orientation: orientation.or(jpeg.orientation),
        ..jpeg
    })
}

/// The size, the header length and the tag of the box at `offset`. `None` at the end of the
/// file and for a box that is smaller than its header or goes past the end.
fn cr3_box_at<R: Read + Seek>(source: &mut Source<R>, offset: u64) -> Option<(u64, u64, [u8; 4])> {
    let header = source.read_at(offset, 16).ok()?;
    let size = u64::from(ByteOrder::Big.u32(&header[0..4])?);
    let (size, head) = match size {
        0 => (source.len - offset, 8),
        1 => (u64::from_be_bytes(header[8..16].try_into().ok()?), 16),
        _ => (size, 8),
    };
    let tag = [header[4], header[5], header[6], header[7]];
    (size >= head && source.holds(offset, size)).then_some((size, head, tag))
}

/// The JPEG in the `PRVW` or `THMB` box body `body[start..end]`. The JPEG starts after a
/// short header, within the first 64 bytes.
fn cr3_jpeg_in<R: Read + Seek>(
    source: &mut Source<R>,
    body: &[u8],
    body_offset: u64,
    start: usize,
    end: usize,
) -> Option<EmbeddedJpeg> {
    let jpeg_start = body[start..end.min(start + 64)]
        .windows(3)
        .position(|w| w == [0xFF, 0xD8, 0xFF])?;
    let file_offset = body_offset + u64::try_from(start + jpeg_start).ok()?;
    source.jpeg_at(file_offset, u64::try_from(end - start - jpeg_start).ok()?)
}

/// The boxes with one of the `tags` anywhere in `data`: the tag and the byte range of the box
/// body. A search by name, because the Canon `uuid` boxes have bytes before their first child.
fn child_boxes(data: &[u8], tags: &[[u8; 4]]) -> Vec<([u8; 4], usize, usize)> {
    let mut found = Vec::new();
    let mut pos = 4;
    while pos + 4 <= data.len() {
        let tag = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
        if tags.contains(&tag)
            && let Some(size) = ByteOrder::Big.u32(&data[pos - 4..pos])
            && let Some(size) = usize::try_from(size).ok().filter(|s| *s >= 8)
            && let Some(end) = (pos - 4).checked_add(size).filter(|end| *end <= data.len())
        {
            found.push((tag, pos + 4, end));
            pos = end + 4;
            continue;
        }
        pos += 1;
    }
    found
}

/// The orientation of IFD0 of the TIFF `data`.
fn tiff_orientation(data: &[u8]) -> Option<u8> {
    let mut source = Source::from_reader(io::Cursor::new(data)).ok()?;
    let (order, first) = tiff_order(&mut source, 0)?;
    let ifd = read_ifd(&mut source, 0, first, order)?;
    ifd.scalar(0x112, order)
        .and_then(|o| u8::try_from(o).ok())
        .filter(|o| (1..=8).contains(o))
}

/// The thumbnail of a Photoshop file: the image resource 1036 (Photoshop 5 and later), a JPEG
/// after a 28-byte header. The older resource 1033 stores BGR and stays out.
fn find_in_psd<R: Read + Seek>(source: &mut Source<R>) -> Option<EmbeddedJpeg> {
    let header = source.read_at(0, 30).ok()?;
    if !header.starts_with(b"8BPS") {
        return None;
    }
    let color_mode_len = u64::from(ByteOrder::Big.u32(&header[26..30])?);
    let resources_start = 30 + color_mode_len;
    let resources_len = u64::from(ByteOrder::Big.u32(&source.read_at(resources_start, 4).ok()?)?);
    if resources_len > MAX_PSD_RESOURCE_BYTES {
        return None;
    }
    let data = source.read_at(resources_start + 4, resources_len).ok()?;
    let mut pos = 0;
    while pos + 11 <= data.len() {
        // Photoshop writes `8BIM`, ImageReady and older tools `MeSa`, `PHUT`, `AgHg` or
        // `DCSR`. The walk takes each resource by its size.
        let id = ByteOrder::Big.u16(&data[pos + 4..pos + 6])?;
        // The name is a Pascal string, padded to an even length with its length byte.
        let name_len = usize::from(data[pos + 6]);
        let name_end = pos + 6 + (name_len + 1).next_multiple_of(2);
        let size = usize::try_from(ByteOrder::Big.u32(data.get(name_end..name_end + 4)?)?).ok()?;
        let body_start = name_end + 4;
        let body_end = body_start
            .checked_add(size)
            .filter(|end| *end <= data.len())?;
        if id == 1036
            && size > 28
            && ByteOrder::Big.u32(&data[body_start..body_start + 4]) == Some(1)
        {
            let jpeg_start = resources_start + 4 + u64::try_from(body_start + 28).ok()?;
            return source.jpeg_at(jpeg_start, u64::try_from(size - 28).ok()?);
        }
        pos = body_end.next_multiple_of(2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{EmbeddedJpeg, decode, decode_covering, find, jpeg_size, may_have_embedded_jpeg};
    use crate::decode::raster::decode_jpeg_bytes;
    use crate::test_support::{
        BLUE, RED, assert_close, red_left_blue_right_jpeg, test_dir, with_exif_orientation,
        write_cr3, write_psd, write_raf, write_tiff_raw,
    };
    use std::fs;
    use std::path::Path;

    fn center(image: &image::DynamicImage) -> [u8; 4] {
        let rgba = image.to_rgba8();
        rgba.get_pixel(rgba.width() / 2, rgba.height() / 2).0
    }

    fn left_right(image: &image::DynamicImage) -> ([u8; 4], [u8; 4]) {
        let rgba = image.to_rgba8();
        let y = rgba.height() / 2;
        (
            rgba.get_pixel(2, y).0,
            rgba.get_pixel(rgba.width() - 3, y).0,
        )
    }

    #[test]
    fn only_raw_and_psd_files_can_carry_an_embedded_jpeg() {
        let may = |name: &str| may_have_embedded_jpeg(Path::new(name));
        assert!(may("a.dng") && may("b.CR3") && may("c.psd") && may("d.raf") && may("e.rw2"));
        assert!(!may("a.pdf") && !may("b.svg") && !may("c.jpg") && !may("d.heic") && !may("e"));
    }

    #[test]
    fn the_sof_segment_gives_the_size_and_rejects_lossless_jpegs() {
        let jpeg = red_left_blue_right_jpeg(40, 24);
        assert_eq!(jpeg_size(&jpeg), Some((40, 24)));
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        assert_eq!(jpeg_size(&jpeg[..10]), None, "cut before the SOF");
        // A lossless JPEG (SOF3), as the sensor data of a DNG.
        let lossless = [0xFF, 0xD8, 0xFF, 0xC3, 0, 11, 8, 0, 4, 0, 4, 1, 1, 0x11, 0];
        assert_eq!(jpeg_size(&lossless), None);
        // Fill bytes before a marker are skipped.
        let mut filled = vec![0xFF, 0xD8, 0xFF, 0xFF];
        filled.extend_from_slice(&jpeg[2..]);
        assert_eq!(jpeg_size(&filled), Some((40, 24)));
    }

    #[test]
    fn the_largest_jpeg_of_a_tiff_raw_wins_and_ifd0_gives_the_orientation() {
        let dir = test_dir();
        let path = dir.path().join("photo.dng");
        // IFD0: a 32x16 JPEG as one strip, orientation 6. SubIFD: a 64x32 JPEG as 0x201/0x202.
        // IFD1: a 16x8 JPEG. The lossless strip of the sensor data stays out.
        write_tiff_raw(&path, 6);
        let jpeg = find(&path).unwrap();
        assert_eq!((jpeg.width, jpeg.height), (64, 32));
        assert_eq!(jpeg.orientation, Some(6));
        assert_eq!(jpeg.shown_size(), (32, 64));
        assert!(jpeg.covers((32, 64)) && !jpeg.covers((33, 10)) && !jpeg.covers((10, 65)));

        // The decode turns the picture: red was left, so it is now at the top.
        let image = decode(&path, &jpeg, None).unwrap();
        assert_eq!((image.width(), image.height()), (32, 64));
        let rgba = image.to_rgba8();
        assert_close(rgba.get_pixel(16, 2).0, RED);
        assert_close(rgba.get_pixel(16, 61).0, BLUE);

        // A target scales the picture down, but never below the target.
        let small = decode(&path, &jpeg, Some((8, 16))).unwrap();
        assert!(small.width() >= 8 && small.height() >= 16 && small.width() < 32);
    }

    #[test]
    fn an_upright_raw_keeps_its_colors_left_and_right() {
        let dir = test_dir();
        let path = dir.path().join("photo.nef");
        write_tiff_raw(&path, 1);
        let jpeg = find(&path).unwrap();
        assert_eq!(jpeg.orientation, Some(1));
        let image = decode(&path, &jpeg, None).unwrap();
        let (left, right) = left_right(&image);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert!(decode_covering(&path, (64, 32)).is_some());
        assert!(
            decode_covering(&path, (65, 32)).is_none(),
            "too small for the box"
        );
    }

    #[test]
    fn a_cr3_takes_the_prvw_box_over_the_thmb_box_with_the_cmt1_orientation() {
        let dir = test_dir();
        let path = dir.path().join("photo.cr3");
        write_cr3(&path, 8);
        let jpeg = find(&path).unwrap();
        assert_eq!((jpeg.width, jpeg.height), (64, 32), "the PRVW box");
        assert_eq!(jpeg.orientation, Some(8));
        let image = decode(&path, &jpeg, None).unwrap();
        assert_eq!((image.width(), image.height()), (32, 64));

        // A bad box after the preview boxes ends the walk, but keeps the JPEG and the
        // orientation.
        let mut data = fs::read(&path).unwrap();
        data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0x00, b'f', b'r', b'e', b'e']);
        data.extend_from_slice(&[0; 8]);
        fs::write(&path, &data).unwrap();
        assert_eq!(find(&path), Some(jpeg), "past the end");
        data.truncate(data.len() - 16);
        data.extend_from_slice(&[0, 0, 0, 4, b'f', b'r', b'e', b'e']);
        data.extend_from_slice(&[0; 8]);
        fs::write(&path, &data).unwrap();
        assert_eq!(find(&path), Some(jpeg), "smaller than its header");
    }

    #[test]
    fn a_raf_reads_the_jpeg_from_its_header() {
        let dir = test_dir();
        let path = dir.path().join("photo.raf");
        write_raf(&path, None);
        let jpeg = find(&path).unwrap();
        assert_eq!((jpeg.width, jpeg.height), (64, 32));
        assert_eq!(jpeg.orientation, None, "no EXIF in the JPEG");
        assert_close(center(&decode(&path, &jpeg, None).unwrap()), RED);

        // A portrait RAF: the EXIF of the JPEG turns it, for the cell shape and the decode.
        let portrait = dir.path().join("portrait.raf");
        write_raf(&portrait, Some(6));
        let jpeg = find(&portrait).unwrap();
        assert_eq!(jpeg.orientation, Some(6));
        assert_eq!(jpeg.shown_size(), (32, 64));
        assert!(jpeg.covers((32, 64)) && !jpeg.covers((64, 32)));
        let image = decode(&portrait, &jpeg, None).unwrap();
        assert_eq!((image.width(), image.height()), (32, 64));
    }

    #[test]
    fn the_container_orientation_wins_over_the_exif_of_the_jpeg() {
        let jpeg = with_exif_orientation(&red_left_blue_right_jpeg(64, 32), 6);
        let turned = decode_jpeg_bytes(&jpeg, None, None).unwrap();
        assert_eq!((turned.width(), turned.height()), (32, 64), "its own EXIF");
        let upright = decode_jpeg_bytes(&jpeg, None, Some(1)).unwrap();
        assert_eq!((upright.width(), upright.height()), (64, 32), "IFD0 says 1");
        let (left, right) = left_right(&upright);
        assert_close(left, RED);
        assert_close(right, BLUE);
    }

    #[test]
    fn a_psd_thumbnail_comes_from_the_resource_1036() {
        let dir = test_dir();
        let path = dir.path().join("design.psd");
        write_psd(&path, true);
        let jpeg = find(&path).unwrap();
        assert_eq!((jpeg.width, jpeg.height), (64, 32));
        let (left, right) = left_right(&decode(&path, &jpeg, None).unwrap());
        assert_close(left, RED);
        assert_close(right, BLUE);

        let plain = dir.path().join("plain.psd");
        write_psd(&plain, false);
        assert_eq!(find(&plain), None, "no thumbnail resource");
    }

    #[test]
    fn broken_files_give_no_jpeg() {
        let dir = test_dir();
        for name in ["a.dng", "b.cr3", "c.raf", "d.psd", "e.arw"] {
            let path = dir.path().join(name);
            fs::write(&path, b"II*\0garbage that is far too short").unwrap();
            assert_eq!(find(&path), None, "{name}");
            fs::write(&path, b"").unwrap();
            assert_eq!(find(&path), None, "{name} empty");
        }
        // A TIFF whose JPEG offsets point outside the file.
        let path = dir.path().join("short.dng");
        write_tiff_raw(&path, 1);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let found = find(&path);
        assert!(
            found.is_none_or(|jpeg| jpeg.offset + jpeg.length <= bytes.len() as u64 / 2),
            "{found:?}"
        );
        assert_eq!(find(&dir.path().join("missing.dng")), None);
        assert!(!may_have_embedded_jpeg(Path::new("a.jpg")));
        let jpeg = EmbeddedJpeg {
            offset: 0,
            length: 10,
            width: 4,
            height: 2,
            orientation: None,
        };
        assert!(decode(&dir.path().join("a.dng"), &jpeg, None).is_err());
    }
}
