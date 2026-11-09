//! The ways to send an opaque RGBA animation frame.

use crate::pictures::kitty::protocol::{self, Keys, write_pixels};
use crate::pictures::kitty::{Pixels, Transfer};
use image::RgbaImage;

/// The cost of the three ways to send an opaque RGBA animation frame through the temp file medium.
#[test]
#[ignore = "measurement: cargo test --release frame_format -- --ignored --nocapture"]
fn timings() {
    for (w, h) in [(400u32, 300u32), (1280, 720), (1920, 1080)] {
        let image = RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
        let keys = Keys {
            first: &|f| format!("a=f,q=2,f={f},s={w},v={h},i=7,z=83,N=1"),
            chunk: "a=f,",
        };
        for name in ["scan and copy", "copy only (scan once)", "as is (RGBA)"] {
            let mut times = Vec::new();
            for _ in 0..200 {
                let mut out = Vec::new();
                let start = std::time::Instant::now();
                let pixels = Pixels::from(&image);
                match name {
                    "as is (RGBA)" => {
                        write_pixels(
                            &mut out,
                            pixels,
                            &keys,
                            Transfer::TempFile,
                            &std::env::temp_dir(),
                        )
                        .unwrap();
                    }
                    "scan and copy" => {
                        let rgb = pixels.opaque_rgb().unwrap();
                        let rgb_pixels = Pixels {
                            alpha: false,
                            data: &rgb,
                            ..pixels
                        };
                        write_pixels(
                            &mut out,
                            rgb_pixels,
                            &keys,
                            Transfer::TempFile,
                            &std::env::temp_dir(),
                        )
                        .unwrap();
                    }
                    _ => {
                        let (source, _) = pixels.data.as_chunks::<4>();
                        let mut rgb = vec![0; source.len() * 3];
                        for (&[r, g, b, _], target) in source.iter().zip(rgb.as_chunks_mut::<3>().0)
                        {
                            *target = [r, g, b];
                        }
                        let rgb_pixels = Pixels {
                            alpha: false,
                            data: &rgb,
                            ..pixels
                        };
                        write_pixels(
                            &mut out,
                            rgb_pixels,
                            &keys,
                            Transfer::TempFile,
                            &std::env::temp_dir(),
                        )
                        .unwrap();
                    }
                }
                times.push(start.elapsed());
                for path in protocol::temp_file_paths(&out) {
                    std::fs::remove_file(path).unwrap();
                }
            }
            times.sort();
            eprintln!("MICRO {w}x{h} {name}: median {:?}", times[100]);
        }
    }
}
