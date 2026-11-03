//! Synthetic Live Photo pairs: a JPEG still with the content identifier in its Apple
//! `MakerNote`, and a movie motion with the identifier in `moov/meta`. No real camera file is
//! in the test set (`docs/live-photos.md`, open question 2).

use super::video::with_temp_file_medium;
use super::wait_until;
use crate::app::App;
use crate::config::Config;
use crate::image_list::SharedImageList;
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};

/// The content identifier of the synthetic pairs.
pub(crate) const LIVE_ID: &str = "8E6B3A55-2C1D-4E0F-9A7B-1F2E3D4C5B6A";

/// Write the still `{stem}.JPG` and the motion `{stem}.MOV` into `dir`, with the identifiers
/// `still` and `motion`. Returns the canonical path of the still.
pub(crate) fn write_live_pair(
    dir: &Path,
    stem: &str,
    still: Option<&str>,
    motion: Option<&str>,
) -> PathBuf {
    let dir = std::fs::canonicalize(dir).unwrap();
    let still_path = dir.join(format!("{stem}.JPG"));
    write_live_still(&still_path, still);
    write_live_motion(&dir.join(format!("{stem}.MOV")), motion);
    still_path
}

/// An app with the temp file medium on the real scan of `dir`, after the end of the scan and
/// of the header read for the cell shape. The grid has 2 rows.
pub(crate) fn scanned_app(dir: &Path, config: &Config) -> App {
    let list = SharedImageList::new();
    let dir = std::fs::canonicalize(dir).unwrap();
    crate::scanner::spawn_with(vec![dir], list.clone(), config.live_photos);
    let mut app = with_temp_file_medium(App::new_with_config((8, 16), list, config));
    app.gallery.zoom_rows = 2;
    app.gallery.update_grid(Rect::new(0, 0, 120, 24));
    wait_until(|| {
        app.refresh_from_scanner();
        app.poll_cell_shape();
        app.scan_complete && !app.cell_shape_pending()
    });
    app
}

/// A scanned folder with the pair `IMG_0001` between two photos, in the fullscreen view of the
/// still.
pub(crate) fn fullscreen_on_still(dir: &Path, motion_id: &str) -> App {
    image::RgbaImage::new(8, 8).save(dir.join("a.png")).unwrap();
    image::RgbaImage::new(8, 8).save(dir.join("z.png")).unwrap();
    write_live_pair(dir, "IMG_0001", Some(LIVE_ID), Some(motion_id));
    let mut app = scanned_app(dir, &Config::default());
    app.fullscreen_box = Rect::new(0, 0, 100, 40);
    app.enter_fullscreen_at(index_of(&app, "IMG_0001.JPG"));
    assert_eq!(app.mode, crate::app::ViewMode::Fullscreen);
    app
}

/// The index of the file `name` in the list of `app`.
pub(crate) fn index_of(app: &App, name: &str) -> usize {
    app.filenames
        .iter()
        .position(|candidate| candidate == name)
        .unwrap()
}

/// A JPEG still. `identifier` goes into the tag 0x0011 of an Apple `MakerNote`, `None` writes no
/// `MakerNote`.
pub(crate) fn write_live_still(path: &Path, identifier: Option<&str>) {
    let jpeg = encoded_jpeg();
    let bytes = match identifier {
        Some(identifier) => with_maker_note(&jpeg, &apple_note(identifier)),
        None => jpeg,
    };
    std::fs::write(path, bytes).unwrap();
}

/// A movie file as the camera writes it: `ftyp`, `mdat`, then `moov` with the identifier.
/// The `mdat` holds no frames: the tests play through `FakeDecoder`.
pub(crate) fn write_live_motion(path: &Path, identifier: Option<&str>) {
    let mut file = mp4_box(*b"ftyp", b"qt  \0\0\0\0qt  ");
    file.extend(mp4_box(*b"mdat", &[0; 64]));
    let meta = identifier.map_or_else(Vec::new, motion_meta);
    file.extend(mp4_box(*b"moov", &meta));
    std::fs::write(path, file).unwrap();
}

/// The color of the synthetic still.
pub(crate) const LIVE_STILL_RGB: [u8; 3] = [40, 120, 200];

/// A plain 64x48 JPEG in `LIVE_STILL_RGB`.
fn encoded_jpeg() -> Vec<u8> {
    let image = image::RgbImage::from_pixel(64, 48, image::Rgb(LIVE_STILL_RGB));
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&image)
        .unwrap();
    out.into_inner()
}

/// The Apple `MakerNote` with one ASCII entry 0x0011 and its text after the IFD.
fn apple_note(identifier: &str) -> Vec<u8> {
    let mut text = identifier.as_bytes().to_vec();
    text.push(0);
    // The header of 14 bytes, the count, one entry of 12 bytes and the next offset of 4 bytes.
    let text_at = 14 + 2 + 12 + 4;
    let mut note = b"Apple iOS\0\0\x01MM".to_vec();
    note.extend_from_slice(&1u16.to_be_bytes());
    note.extend_from_slice(&0x0011u16.to_be_bytes());
    note.extend_from_slice(&2u16.to_be_bytes());
    note.extend_from_slice(&u32::try_from(text.len()).unwrap().to_be_bytes());
    note.extend_from_slice(&u32::try_from(text_at).unwrap().to_be_bytes());
    note.extend_from_slice(&0u32.to_be_bytes());
    note.extend(text);
    note
}

/// `jpeg` with an APP1 EXIF segment that holds `note` as the tag `MakerNote`.
fn with_maker_note(jpeg: &[u8], note: &[u8]) -> Vec<u8> {
    let mut writer = exif::experimental::Writer::new();
    let field = exif::Field {
        tag: exif::Tag::MakerNote,
        ifd_num: exif::In::PRIMARY,
        value: exif::Value::Undefined(note.to_vec(), 0),
    };
    writer.push_field(&field);
    let mut tiff = std::io::Cursor::new(Vec::new());
    writer.write(&mut tiff, true).unwrap();
    let mut segment = b"Exif\0\0".to_vec();
    segment.extend(tiff.into_inner());
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&u16::try_from(segment.len() + 2).unwrap().to_be_bytes());
    out.extend(segment);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// A `meta` box of the movie format, without version and flags: `hdlr`, `keys` and `ilst`.
fn motion_meta(identifier: &str) -> Vec<u8> {
    let mut handler = vec![0; 8];
    handler.extend_from_slice(b"mdta");
    handler.extend_from_slice(&[0; 13]);
    let key = b"com.apple.quicktime.content.identifier";
    // One key before the identifier, so that the index is 2.
    let other = b"com.apple.quicktime.make";
    let mut keys = vec![0, 0, 0, 0];
    keys.extend_from_slice(&2u32.to_be_bytes());
    keys.extend(mp4_box(*b"mdta", other));
    keys.extend(mp4_box(*b"mdta", key));
    let mut data = 1u32.to_be_bytes().to_vec();
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(identifier.as_bytes());
    let mut make = 1u32.to_be_bytes().to_vec();
    make.extend_from_slice(&0u32.to_be_bytes());
    make.extend_from_slice(b"Apple");
    let mut list = mp4_box(1u32.to_be_bytes(), &mp4_box(*b"data", &make));
    list.extend(mp4_box(2u32.to_be_bytes(), &mp4_box(*b"data", &data)));
    let mut meta = mp4_box(*b"hdlr", &handler);
    meta.extend(mp4_box(*b"keys", &keys));
    meta.extend(mp4_box(*b"ilst", &list));
    mp4_box(*b"meta", &meta)
}

fn mp4_box(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(body.len() + 8)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(&kind);
    out.extend_from_slice(body);
    out
}

/// The folder of real Live Photo files in `PHEWR_TEST_LIVE_DIR`, or `None` with a note: the
/// test skips. Its `README.md` lists the cases.
pub(crate) fn real_live_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PHEWR_TEST_LIVE_DIR").map(PathBuf::from);
    if dir.is_none() {
        println!("note: skipped, because PHEWR_TEST_LIVE_DIR is not set");
    }
    dir
}
