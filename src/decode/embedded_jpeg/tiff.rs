//! The JPEGs of a TIFF-based RAW file: the `JPEGInterchangeFormat` pair (0x201, 0x202), the
//! strip of a JPEG-compressed image, or the Panasonic `JpgFromRaw` tag (0x2e).

use super::{ByteOrder, EmbeddedJpeg, Source};
use std::collections::HashSet;
use std::io::{self, Read, Seek};

/// A RAW file has 2 to 6 IFDs.
const MAX_IFDS: usize = 32;

/// A maker note IFD has a few hundred entries.
const MAX_IFD_ENTRIES: u16 = 1024;

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

    /// The value of a SHORT or LONG entry with count 1.
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

    /// The data offset of an entry larger than its 4 value bytes.
    fn data_offset(&self, order: ByteOrder) -> Option<u32> {
        order.u32(&self.value)
    }
}

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

/// The byte order and the IFD0 offset. The magic is 42 for TIFF and DNG, `RO` and `RS` for
/// Olympus ORF and 85 for Panasonic RW2.
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

/// The `SubIFDs` offsets (tag 0x14a): inline for one, else in the data.
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

/// The largest JPEG of IFD0, its `SubIFDs` and the IFD chain, with the IFD0 orientation.
pub(super) fn find_in_tiff<R: Read + Seek>(
    source: &mut Source<R>,
    base: u64,
) -> Option<EmbeddedJpeg> {
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

pub(super) fn tiff_orientation(data: &[u8]) -> Option<u8> {
    let mut source = Source::from_reader(io::Cursor::new(data)).ok()?;
    let (order, first) = tiff_order(&mut source, 0)?;
    let ifd = read_ifd(&mut source, 0, first, order)?;
    ifd.scalar(0x112, order)
        .and_then(|o| u8::try_from(o).ok())
        .filter(|o| (1..=8).contains(o))
}
