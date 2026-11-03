//! The content identifiers of a Live Photo pair. The camera writes one UUID into the still and
//! into the motion.
//!
//! - The still: the ASCII tag 0x0011 of the Apple `MakerNote` (EXIF tag 0x927C), in a big-endian
//!   IFD at byte 14 of the note.
//! - The motion: the `mdta` key `com.apple.quicktime.content.identifier` in `moov/meta`.
//!
//! A read costs 0.02 ms to 0.05 ms per file from the page cache and 0.35 ms on the first read.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const STILL_TAG: u16 = 0x0011;
const APPLE_NOTE: &[u8] = b"Apple iOS\0";
/// The IFD of the note starts at byte 14. Its offsets count from the start of the note.
const NOTE_IFD: usize = 14;
const ASCII: u16 = 2;
const MOTION_KEY: &[u8] = b"com.apple.quicktime.content.identifier";
/// The `moov` box of a Live Photo motion holds about 6 kB. A plain clip with the base name of a
/// still can hold several MB (2.9 MB for 16 GB of iPhone video). A larger box is no Live Photo,
/// and the read on the UI thread stays below 1 MB.
const MAX_MOOV_BYTES: u64 = 1 << 20;

/// True when the still and the motion both carry an identifier and the identifiers differ. A
/// file without an identifier counts as a match: the base name paired it.
pub(crate) fn identifiers_differ(still: &Path, motion: &Path) -> bool {
    let (Some(still), Some(motion)) = (still_identifier(still), motion_identifier(motion)) else {
        return false;
    };
    !still.eq_ignore_ascii_case(&motion)
}

/// The tag 0x0011 of the Apple `MakerNote`, in a HEIC `Exif` item or a JPEG APP1 segment.
pub(super) fn still_identifier(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;
    let field = exif.get_field(exif::Tag::MakerNote, exif::In::PRIMARY)?;
    let exif::Value::Undefined(note, _) = &field.value else {
        return None;
    };
    apple_ascii_tag(note, STILL_TAG)
}

/// An ASCII tag of the Apple `MakerNote`. The note starts with `Apple iOS\0`, a version of 2
/// bytes and the byte order `MM`.
fn apple_ascii_tag(note: &[u8], wanted: u16) -> Option<String> {
    if !note.starts_with(APPLE_NOTE) || note.get(12..NOTE_IFD)? != b"MM" {
        return None;
    }
    let count = usize::from(be_u16(note, NOTE_IFD)?);
    let entry = (0..count)
        .map(|index| NOTE_IFD + 2 + index * 12)
        .find(|&entry| be_u16(note, entry) == Some(wanted))?;
    if be_u16(note, entry + 2)? != ASCII {
        return None;
    }
    let len = usize::try_from(be_u32(note, entry + 4)?).ok()?;
    let start = if len <= 4 {
        entry + 8
    } else {
        usize::try_from(be_u32(note, entry + 8)?).ok()?
    };
    let text = note.get(start..start.checked_add(len)?)?;
    let text = text.split(|&byte| byte == 0).next()?;
    String::from_utf8(text.to_vec()).ok()
}

/// The `mdta` key of the identifier in `moov/meta`. The walk seeks over `mdat`.
pub(super) fn motion_identifier(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let moov = top_level_box(&mut file, *b"moov")?;
    let meta = child(&moov, *b"meta")?;
    // A QuickTime `meta` box has no version and no flags, an ISO `meta` box has 4 bytes of them.
    let meta = if meta.get(4..8) == Some(&b"hdlr"[..]) {
        meta
    } else {
        meta.get(4..)?
    };
    let index = key_index(child(meta, *b"keys")?, MOTION_KEY)?;
    let item = child(child(meta, *b"ilst")?, index.to_be_bytes())?;
    // The `data` box holds a type (1 is UTF-8) and a locale of 4 bytes each before the value.
    let data = child(item, *b"data")?;
    String::from_utf8(data.get(8..)?.to_vec()).ok()
}

/// The 1-based index of the `mdta` key `wanted` in a `keys` box. An `ilst` item has the index
/// as its box type.
fn key_index(keys: &[u8], wanted: &[u8]) -> Option<u32> {
    let count = be_u32(keys, 4)?;
    let mut at = 8;
    for index in 1..=count {
        let size = usize::try_from(be_u32(keys, at)?).ok()?;
        if size < 8 {
            return None;
        }
        if keys.get(at + 4..at + 8)? == b"mdta" && keys.get(at + 8..at + size)? == wanted {
            return Some(index);
        }
        at += size;
    }
    None
}

/// The body of the first top-level box of the type `wanted`. A live photo motion puts `moov`
/// after `mdat`, so the walk seeks over the boxes before it.
fn top_level_box(file: &mut File, wanted: [u8; 4]) -> Option<Vec<u8>> {
    loop {
        let mut header = [0; 8];
        file.read_exact(&mut header).ok()?;
        let body_len = match be_u32(&header, 0)? {
            1 => {
                let mut large = [0; 8];
                file.read_exact(&mut large).ok()?;
                u64::from_be_bytes(large).checked_sub(16)?
            }
            0 => return None,
            size => u64::from(size).checked_sub(8)?,
        };
        if header[4..] == wanted {
            if body_len > MAX_MOOV_BYTES {
                return None;
            }
            let mut body = vec![0; usize::try_from(body_len).ok()?];
            file.read_exact(&mut body).ok()?;
            return Some(body);
        }
        file.seek(SeekFrom::Current(i64::try_from(body_len).ok()?))
            .ok()?;
    }
}

/// The body of the first child box of the type `wanted`.
fn child(data: &[u8], wanted: [u8; 4]) -> Option<&[u8]> {
    boxes(data)
        .find(|&(kind, _)| kind == wanted)
        .map(|(_, body)| body)
}

/// The child boxes as `(type, body)`. The walk ends at the first box that does not fit.
fn boxes(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut rest = data;
    std::iter::from_fn(move || {
        let size = usize::try_from(be_u32(rest, 0)?).ok()?;
        let kind: [u8; 4] = rest.get(4..8)?.try_into().ok()?;
        let (size, header) = match size {
            0 => (rest.len(), 8),
            1 => (
                usize::try_from(u64::from_be_bytes(rest.get(8..16)?.try_into().ok()?)).ok()?,
                16,
            ),
            size => (size, 8),
        };
        let body = rest.get(header..size)?;
        rest = rest.get(size..)?;
        Some((kind, body))
    })
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}
