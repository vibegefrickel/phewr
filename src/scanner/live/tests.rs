//! The tests of the base-name pairs and of the content identifiers.

use super::pair;
use crate::scanner::name_key;
use std::path::{Path, PathBuf};

/// The paths of `names` below `/photos`, sorted like the scan.
fn sorted(names: &[&str]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = names
        .iter()
        .map(|name| Path::new("/photos").join(name))
        .collect();
    files.sort_by_cached_key(|path| name_key(path));
    files
}

fn names(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn photo(name: &str) -> PathBuf {
    Path::new("/photos").join(name)
}

#[test]
fn hides_motion_of_each_still() {
    let mut files = sorted(&[
        "IMG_0001.HEIC",
        "IMG_0001.MOV",
        "IMG_0002.JPG",
        "IMG_0002.MOV",
        "IMG_0003.MOV",
        "a.png",
    ]);
    let live = pair(&mut files);

    assert_eq!(
        names(&files),
        ["a.png", "IMG_0001.HEIC", "IMG_0002.JPG", "IMG_0003.MOV"],
        "a movie without a still stays a cell"
    );
    assert_eq!(live.len(), 2);
    assert_eq!(
        live.motion(&photo("IMG_0001.HEIC")),
        Some(photo("IMG_0001.MOV").as_path())
    );
    assert_eq!(
        live.motion(&photo("IMG_0002.JPG")),
        Some(photo("IMG_0002.MOV").as_path())
    );
    assert_eq!(live.motion(&photo("a.png")), None);
    assert_eq!(
        live.still_of(&photo("IMG_0002.MOV")),
        Some(photo("IMG_0002.JPG").as_path())
    );
    assert_eq!(live.still_of(&photo("IMG_0003.MOV")), None);
}

#[test]
fn heic_wins_over_jpeg_twin() {
    let mut files = sorted(&["IMG_1234.JPG", "IMG_1234.MOV", "IMG_1234.HEIC"]);
    let live = pair(&mut files);

    assert_eq!(names(&files), ["IMG_1234.HEIC", "IMG_1234.JPG"]);
    assert_eq!(
        live.motion(&photo("IMG_1234.HEIC")),
        Some(photo("IMG_1234.MOV").as_path())
    );
    assert_eq!(live.motion(&photo("IMG_1234.JPG")), None);
}

#[test]
fn edited_copy_pairs_with_own_motion() {
    let mut files = sorted(&[
        "IMG_1234.HEIC",
        "IMG_E1234.HEIC",
        "IMG_E1234.MOV",
        "IMG_E5678.HEIC",
        "IMG_5678.MOV",
    ]);
    let live = pair(&mut files);

    assert_eq!(
        names(&files),
        [
            "IMG_1234.HEIC",
            "IMG_5678.MOV",
            "IMG_E1234.HEIC",
            "IMG_E5678.HEIC"
        ],
        "IMG_5678.MOV has no still of its own"
    );
    assert_eq!(live.motion(&photo("IMG_1234.HEIC")), None);
    assert_eq!(
        live.motion(&photo("IMG_E1234.HEIC")),
        Some(photo("IMG_E1234.MOV").as_path())
    );
    assert_eq!(live.motion(&photo("IMG_E5678.HEIC")), None);
}

#[test]
fn case_and_neighbors() {
    let mut files = sorted(&[
        "img_0001.heic",
        "IMG_0001.live.heic",
        "IMG_0001.Mov",
        "IMG_0002.HEIC",
        "IMG_0002.mp4",
    ]);
    let live = pair(&mut files);

    assert_eq!(
        live.motion(&photo("img_0001.heic")),
        Some(photo("IMG_0001.Mov").as_path()),
        "the stems match without regard to case, behind one more name"
    );
    assert_eq!(
        live.motion(&photo("IMG_0002.HEIC")),
        None,
        "only a .mov is a motion"
    );
    assert_eq!(files.len(), 4);
}

#[test]
fn still_pairs_once() {
    // Two movies that differ in case only, on a case-sensitive volume.
    let mut files = sorted(&["IMG_0001.HEIC", "IMG_0001.MOV", "IMG_0001.mov"]);
    let live = pair(&mut files);

    assert_eq!(live.len(), 1);
    assert_eq!(files.len(), 2, "the second movie stays a cell");
}

#[test]
fn no_movie_keeps_list() {
    let mut files = sorted(&["a.jpg", "b.heic"]);
    let before = files.clone();
    assert!(pair(&mut files).is_empty());
    assert_eq!(files, before);
}

/// The best of 20 passes over `files`, and the pairs that it found.
fn best_pass(files: &[PathBuf]) -> (std::time::Duration, usize) {
    let mut best = std::time::Duration::MAX;
    let mut found = 0;
    for _ in 0..20 {
        let mut copy = files.to_vec();
        let start = std::time::Instant::now();
        found = pair(&mut copy).len();
        best = best.min(start.elapsed());
    }
    (best, found)
}

/// The names of `index` in a layout: `plain` holds no movie, `mixed` a movie in each tenth
/// name without a still, and `pairs` a pair and a JPEG in each third name.
fn layout_names(layout: &str, index: usize) -> Vec<String> {
    match layout {
        "mixed" if index.is_multiple_of(10) => vec![format!("IMG_{index:05}.MOV")],
        "pairs" if index.is_multiple_of(3) => vec![
            format!("IMG_{index:05}.HEIC"),
            format!("IMG_{index:05}.MOV"),
            format!("IMG_{index:05}x.JPG"),
        ],
        "pairs" => Vec::new(),
        _ => vec![format!("IMG_{index:05}.HEIC")],
    }
}

/// The pass over 10 000 sorted names in each layout of `layout_names`.
#[test]
#[ignore = "timing"]
fn bench_pair_pass() {
    for layout in ["plain", "mixed", "pairs"] {
        let names: Vec<String> = (0..10_002)
            .flat_map(|index| layout_names(layout, index))
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let files = sorted(&names);
        let (best, found) = best_pass(&files);
        eprintln!(
            "bench pair pass: {layout}, {} names, {found} pairs, {best:?}",
            files.len()
        );
    }
}

#[cfg(feature = "video")]
mod identifiers {
    use super::super::identifier::{motion_identifier, still_identifier};
    use super::super::identifiers_differ;
    use crate::test_support::live::{write_live_motion, write_live_still};

    const ID: &str = "8E6B3A55-2C1D-4E0F-9A7B-1F2E3D4C5B6A";
    const OTHER: &str = "0F1E2D3C-4B5A-4968-8776-A5B4C3D2E1F0";

    #[test]
    fn reads_both_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let (still, motion) = (dir.path().join("a.jpg"), dir.path().join("a.mov"));
        write_live_still(&still, Some(ID));
        write_live_motion(&motion, Some(ID));

        assert_eq!(still_identifier(&still).as_deref(), Some(ID));
        assert_eq!(motion_identifier(&motion).as_deref(), Some(ID));
        assert!(!identifiers_differ(&still, &motion));

        write_live_motion(&motion, Some(&ID.to_lowercase()));
        assert!(!identifiers_differ(&still, &motion), "a UUID ignores case");
    }

    #[test]
    fn large_moov_is_no_live_photo() {
        let dir = tempfile::tempdir().unwrap();
        let motion = dir.path().join("a.mov");
        // A `moov` header that claims 1 MB and one byte of body: the read stops at the header.
        let mut file = u32::try_from((1 << 20) + 9).unwrap().to_be_bytes().to_vec();
        file.extend_from_slice(b"moov");
        std::fs::write(&motion, file).unwrap();
        assert_eq!(motion_identifier(&motion), None);
    }

    /// The read of the identifier of `PHEWR_TEST_MOTION`, a clip of any size: the cost after
    /// the cap of the `moov` box.
    #[test]
    #[ignore = "timing: needs PHEWR_TEST_MOTION"]
    fn bench_motion_identifier() {
        let Some(path) = std::env::var_os("PHEWR_TEST_MOTION") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        for run in 0..3 {
            let start = std::time::Instant::now();
            let id = motion_identifier(&path);
            println!("run {run}: {id:?} in {:?}", start.elapsed());
        }
    }

    #[test]
    fn mismatch_needs_both() {
        let dir = tempfile::tempdir().unwrap();
        let (still, motion) = (dir.path().join("a.jpg"), dir.path().join("a.mov"));
        write_live_still(&still, Some(ID));
        write_live_motion(&motion, Some(OTHER));
        assert!(identifiers_differ(&still, &motion));

        write_live_still(&still, None);
        assert_eq!(still_identifier(&still), None);
        assert!(
            !identifiers_differ(&still, &motion),
            "a still without the tag trusts the name"
        );

        write_live_still(&still, Some(ID));
        write_live_motion(&motion, None);
        assert_eq!(motion_identifier(&motion), None);
        assert!(!identifiers_differ(&still, &motion));
        assert!(!identifiers_differ(&dir.path().join("gone.jpg"), &motion));
    }
}

/// The identifiers of the real files of `PHEWR_TEST_LIVE_DIR` match the values of exiftool.
#[cfg(feature = "video")]
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR"]
fn real_identifiers_match_exiftool() {
    use super::identifier::{motion_identifier, still_identifier};
    use super::identifiers_differ;
    let Some(dir) = crate::test_support::live::real_live_dir() else {
        return;
    };
    let id = |name: &str| {
        let path = dir.join(name);
        let start = std::time::Instant::now();
        let id = if super::is_motion(&path) {
            motion_identifier(&path)
        } else {
            still_identifier(&path)
        };
        println!("{name}: {id:?} in {:?}", start.elapsed());
        id
    };
    // The values that `exiftool -Apple:ContentIdentifier -Keys:ContentIdentifier` reads.
    let expected = [
        ("IMG_0711.HEIC", "0FD24349-014A-423D-AE8D-35FCE43A24E7"),
        ("IMG_2927.HEIC", "1745E022-9712-43C7-B024-F308EC6413CB"),
        ("IMG_2927.MOV", "1745E022-9712-43C7-B024-F308EC6413CB"),
        ("IMG_2928.HEIC", "56AA36A8-98DB-496E-B2D0-5749C32EC49B"),
        ("IMG_2928.MOV", "56AA36A8-98DB-496E-B2D0-5749C32EC49B"),
        ("IMG_2929.HEIC", "ED69AFC7-A2EB-4FE9-88A0-54293FC2478F"),
        ("IMG_2929.MOV", "EE6D649E-788F-4E3A-BCD2-483651BF7B34"),
        ("IMG_4410.MOV", "EE6D649E-788F-4E3A-BCD2-483651BF7B34"),
        ("IMG_5001.JPG", "56AA36A8-98DB-496E-B2D0-5749C32EC49B"),
        ("IMG_5001.MOV", "56AA36A8-98DB-496E-B2D0-5749C32EC49B"),
        ("IMG_E2928.HEIC", "56AA36A8-98DB-496E-B2D0-5749C32EC49B"),
    ];
    for (name, want) in expected {
        assert_eq!(id(name).as_deref(), Some(want), "{name}");
    }
    for (stem, still, differ) in [
        ("IMG_2927", "IMG_2927.HEIC", false),
        ("IMG_2928", "IMG_2928.HEIC", false),
        ("IMG_5001", "IMG_5001.JPG", false),
        ("IMG_2929", "IMG_2929.HEIC", true),
    ] {
        let motion = dir.join(format!("{stem}.MOV"));
        assert_eq!(
            identifiers_differ(&dir.join(still), &motion),
            differ,
            "{stem}"
        );
    }
}
