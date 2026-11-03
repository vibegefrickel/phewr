//! The facts that the player needs before the first frame: the size after the rotation, the
//! frame rate, the duration, the HDR curve and the bit depth.
//!
//! One `ffprobe` process reads them from the headers and writes JSON. An animated image (GIF,
//! WebP, APNG, AVIS) takes its rate and its length from the packet durations. Its container
//! tells no frame rate that fits the frame delays.

mod parse;

use super::tone_map::{Hdr, Primaries};
use lru::LruCache;
use parse::parse;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// The header facts of a video. The size is the size on the screen, after the rotation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// The clockwise rotation of the display matrix: 0, 90, 180 or 270 degrees.
    pub rotation: u16,
    /// The pacing rate: `avg_frame_rate`, `r_frame_rate` for a variable rate, or the packet
    /// durations of an animation.
    pub fps: f64,
    /// 0 when the container does not tell the length.
    pub duration: f64,
    pub hdr: Option<Hdr>,
    /// An untagged stream is BT.709, as sRGB.
    pub primaries: Primaries,
    /// The hardware decoder gives back `p010le` frames for a 10-bit stream.
    pub ten_bit: bool,
    pub rate: Rate,
    /// The stream that ffmpeg maps. An AVIS file holds its still item first and its sequence
    /// second.
    pub stream: usize,
}

/// How ffmpeg gets the output rate `fps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rate {
    /// ffmpeg writes the frames as they are.
    Constant,
    /// ffmpeg duplicates or drops frames to the constant rate `fps`, see
    /// `parse::variable_fps`.
    Variable,
    /// ffmpeg decodes it in software at the exact rate `fps`.
    Animation,
}

impl VideoInfo {
    pub(crate) fn is_animation(&self) -> bool {
        self.rate == Rate::Animation
    }

    pub(crate) fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps)
    }
}

/// The packets of the timing walk. The walk runs on the UI thread before the first frame.
const TIMING_PACKETS: usize = 16;

/// One packet more than [`TIMING_PACKETS`] tells whether the walk reached the end.
const PACKET_LIMIT: usize = TIMING_PACKETS + 1;

/// `-read_intervals` counts the packets of all video streams, and an AVIS file interleaves its
/// still item with its sequence.
const AVIS_PACKET_LIMIT: usize = 2 * PACKET_LIMIT;

/// A file on a stalled volume must not hold the UI thread or a thumbnail slot for longer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const PROBE_CACHE_ENTRIES: usize = 1024;

const STREAM_ENTRIES: &str = "stream=index,width,height,pix_fmt,color_transfer,color_primaries,\
    r_frame_rate,avg_frame_rate,time_base,nb_frames:stream_disposition=default,attached_pic:\
    stream_side_data=side_data_type,rotation:format=duration";

/// `None` for a file without an animation extension.
fn packet_limit(path: &Path) -> Option<usize> {
    if !super::animation::has_animation_extension(path) {
        return None;
    }
    let avif = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("avif"));
    Some(if avif {
        AVIS_PACKET_LIMIT
    } else {
        PACKET_LIMIT
    })
}

/// Only an animation reads packets, at most `packet_limit`.
fn probe_args(video: &Path, packet_limit: Option<usize>) -> Vec<OsString> {
    let entries = if packet_limit.is_some() {
        format!("{STREAM_ENTRIES}:packet=stream_index,duration")
    } else {
        STREAM_ENTRIES.to_owned()
    };
    let mut args: Vec<OsString> = ["-v", "error", "-select_streams", "v", "-of", "json"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.extend([OsString::from("-show_entries"), OsString::from(entries)]);
    if let Some(limit) = packet_limit {
        args.extend([
            OsString::from("-read_intervals"),
            OsString::from(format!("%+#{limit}")),
        ]);
    }
    args.extend([OsString::from("-i"), video.as_os_str().to_owned()]);
    args
}

/// Read the header of `path` with `ffprobe`. No frame is decoded.
pub(crate) fn probe(program: &Path, path: &Path) -> io::Result<VideoInfo> {
    let packet_limit = packet_limit(path);
    let mut command = Command::new(program);
    command.args(probe_args(path, packet_limit));
    let output = output_within(command, PROBE_TIMEOUT)?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        let reason = text.lines().map(str::trim).rfind(|line| !line.is_empty());
        return Err(invalid(reason.unwrap_or("ffprobe could not read the file")));
    }
    let mut info = parse(&output.stdout, packet_limit)?;
    // ffprobe reports no profile for an animated WebP or APNG. The file can carry one.
    if info.is_animation()
        && info.hdr.is_none()
        && info.primaries == Primaries::Bt709
        && let Some(primaries) = super::animation::embedded_primaries(path)
    {
        info.primaries = primaries;
    }
    Ok(info)
}

/// The thumbnail job probes each video of the grid. `Player::open` then starts no `ffprobe`.
pub(crate) fn probe_cached(program: &Path, path: &Path) -> io::Result<VideoInfo> {
    static CACHE: LazyLock<ProbeCache> = LazyLock::new(|| ProbeCache::new(PROBE_CACHE_ENTRIES));
    CACHE.get_or_probe(path, || probe(program, path))
}

/// The probes of the session, keyed by path. A changed mtime or length probes again.
struct ProbeCache {
    state: Mutex<CacheState>,
    /// Wakes the callers that wait for a probe of their path.
    probed: Condvar,
}

struct CacheState {
    entries: LruCache<PathBuf, (FileStamp, VideoInfo)>,
    /// The paths whose ffprobe runs now.
    probing: HashSet<PathBuf>,
}

type FileStamp = (Option<SystemTime>, u64);

impl ProbeCache {
    fn new(capacity: usize) -> Self {
        let capacity = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            state: Mutex::new(CacheState {
                entries: LruCache::new(capacity),
                probing: HashSet::new(),
            }),
            probed: Condvar::new(),
        }
    }

    /// Only a success goes into the cache. A second caller for the same path waits for the
    /// running probe, and the lock stays free while ffprobe runs.
    fn get_or_probe(
        &self,
        path: &Path,
        run: impl FnOnce() -> io::Result<VideoInfo>,
    ) -> io::Result<VideoInfo> {
        let metadata = std::fs::metadata(path)?;
        let stamp = (metadata.modified().ok(), metadata.len());
        let mut state = self.lock();
        loop {
            let cached = state
                .entries
                .get(path)
                .filter(|(cached_stamp, _)| *cached_stamp == stamp);
            if let Some((_, info)) = cached {
                return Ok(info.clone());
            }
            if !state.probing.contains(path) {
                break;
            }
            state = self
                .probed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.probing.insert(path.to_path_buf());
        drop(state);
        let probing = Probing { cache: self, path };
        let info = run()?;
        probing
            .cache
            .lock()
            .entries
            .put(path.to_path_buf(), (stamp, info.clone()));
        Ok(info)
    }

    /// A thumbnail job that panicked leaves the entries usable.
    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Ends the probe of `path` also on an error or a panic: no caller waits forever.
struct Probing<'a> {
    cache: &'a ProbeCache,
    path: &'a Path,
}

impl Drop for Probing<'_> {
    fn drop(&mut self) {
        self.cache.lock().probing.remove(self.path);
        self.cache.probed.notify_all();
    }
}

/// `Command::output` with a timeout. A longer run is killed, and the error is `TimedOut`.
pub(crate) fn output_within(mut command: Command, timeout: Duration) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = read_on_thread(child.stdout.take());
    let stderr = read_on_thread(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a process that ended needs no kill"
            )]
            let _ = child.kill();
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the wait only reaps the process"
            )]
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the program did not finish in time",
            ));
        }
        thread::sleep(OUTPUT_POLL);
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// A probe takes about 30 ms.
const OUTPUT_POLL: Duration = Duration::from_millis(1);

/// A full pipe must never block the child.
fn read_on_thread(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a read error keeps the bytes so far, and the exit status tells the rest"
            )]
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

fn invalid(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{ProbeCache, Rate, output_within, packet_limit, probe, probe_args};
    use crate::test_support::fixture;
    use crate::test_support::video::real_ffprobe;
    use std::cell::Cell;
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn only_animation_reads_packets() {
        let text = |args: Vec<std::ffi::OsString>| -> Vec<String> {
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        let video = text(probe_args(Path::new("-clip.mov"), None));
        assert_eq!(
            video[..6],
            ["-v", "error", "-select_streams", "v", "-of", "json"]
        );
        assert_eq!(video[video.len() - 2..], ["-i", "-clip.mov"]);
        assert!(!video.iter().any(|arg| arg.contains("packet")), "{video:?}");
        assert!(!video.contains(&"-read_intervals".to_owned()));
        let animation = text(probe_args(Path::new("anim.gif"), Some(17)));
        assert!(
            animation[7].ends_with(":packet=stream_index,duration"),
            "{animation:?}"
        );
        assert_eq!(animation[8..10], ["-read_intervals", "%+#17"]);
    }

    #[test]
    fn avif_reads_packets_for_two_streams() {
        assert_eq!(packet_limit(Path::new("clip.mov")), None);
        assert_eq!(packet_limit(Path::new("still.jpg")), None);
        for name in ["a.gif", "b.webp", "c.png", "d.APNG"] {
            assert_eq!(packet_limit(Path::new(name)), Some(17), "{name}");
        }
        assert_eq!(packet_limit(Path::new("e.avif")), Some(34));
        assert_eq!(packet_limit(Path::new("f.AVIF")), Some(34));
    }

    #[test]
    fn cache_probes_until_file_changes() {
        let cache = ProbeCache::new(8);
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "clip").unwrap();
        let runs = Cell::new(0);
        let run = || {
            runs.set(runs.get() + 1);
            Ok(crate::test_support::video::clip_info())
        };
        cache.get_or_probe(file.path(), run).unwrap();
        cache.get_or_probe(file.path(), run).unwrap();
        assert_eq!(runs.get(), 1, "the second probe is a hit");
        std::fs::write(file.path(), "a longer clip").unwrap();
        cache.get_or_probe(file.path(), run).unwrap();
        assert_eq!(runs.get(), 2, "a changed length probes again");
        let failed = cache.get_or_probe(Path::new("/nonexistent/a.mov"), run);
        assert!(
            failed.is_err() && runs.get() == 2,
            "a missing file starts no probe"
        );
        let error = || Err(std::io::Error::other("no video stream"));
        let other = tempfile::NamedTempFile::new().unwrap();
        assert!(cache.get_or_probe(other.path(), error).is_err());
        cache.get_or_probe(other.path(), run).unwrap();
        assert_eq!(runs.get(), 3, "an error does not go into the cache");
    }

    /// The first probe holds until the second caller had 100 ms to start its own ffprobe.
    #[test]
    fn second_caller_waits_for_running_probe() {
        let cache = ProbeCache::new(8);
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "clip").unwrap();
        let second_runs = AtomicUsize::new(0);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (cache, path) = (&cache, file.path());
        std::thread::scope(|scope| {
            scope.spawn(move || {
                cache.get_or_probe(path, || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(crate::test_support::video::clip_info())
                })
            });
            started_rx.recv().unwrap();
            let second = scope.spawn(|| {
                cache.get_or_probe(path, || {
                    second_runs.fetch_add(1, Ordering::SeqCst);
                    Ok(crate::test_support::video::clip_info())
                })
            });
            for _ in 0..10 {
                if second_runs.load(Ordering::SeqCst) > 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            release_tx.send(()).unwrap();
            assert!(second.join().unwrap().is_ok());
        });
        assert_eq!(
            second_runs.load(Ordering::SeqCst),
            0,
            "one ffprobe per path"
        );
    }

    #[test]
    fn panicked_probe_frees_path() {
        let cache = ProbeCache::new(8);
        let file = tempfile::NamedTempFile::new().unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.get_or_probe(file.path(), || panic!("synthetic probe panic"))
        }));
        assert!(panicked.is_err());
        let info = cache.get_or_probe(file.path(), || Ok(crate::test_support::video::clip_info()));
        assert!(info.is_ok(), "the next caller probes and does not wait");
    }

    #[test]
    fn hanging_program_times_out() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let started = std::time::Instant::now();
        let error = output_within(command, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the kill ends the wait"
        );

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf out; printf err >&2; exit 3"]);
        let output = output_within(command, Duration::from_secs(10)).unwrap();
        assert_eq!(
            (output.stdout, output.stderr),
            (b"out".to_vec(), b"err".to_vec())
        );
        assert_eq!(output.status.code(), Some(3));
    }

    /// A variable and a constant-rate clip that the real ffmpeg writes.
    #[test]
    fn real_clips_detect_variable_rate() {
        let (Some(program), Some(ffprobe)) =
            (crate::test_support::video::real_ffmpeg(), real_ffprobe())
        else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        for (name, variable) in [
            ("vfr.mp4", true),
            ("vfr.mov", true),
            ("cfr.mp4", false),
            ("cfr.mov", false),
        ] {
            let path = dir.path().join(name);
            let Some(path) = crate::test_support::video::write_clip(&program, &path, variable)
            else {
                return;
            };
            let info = probe(&ffprobe, &path).unwrap();
            let kind = if variable {
                Rate::Variable
            } else {
                Rate::Constant
            };
            assert_eq!(info.rate, kind, "{name}: {info:?}");
            // Bursts at 60 and an average of 24.7: the clip plays at 24.7.
            let fps = if variable { 2400.0 / 97.0 } else { 24.0 };
            assert!((info.fps - fps).abs() < 1e-9, "{name}: {info:?}");
            assert!(!info.is_animation(), "{name}");
        }
    }

    /// The animated fixtures: 3 frames of 16x12 pixels each.
    #[test]
    fn real_animation_timing() {
        let Some(ffprobe) = real_ffprobe() else {
            return;
        };
        // The GIF has delays of 100, 500 and 200 ms: the rate is 10 and the length 0.8.
        let gif = probe(&ffprobe, &fixture("anim.gif")).unwrap();
        assert!(gif.is_animation(), "{gif:?}");
        assert_eq!((gif.width, gif.height, gif.stream), (16, 12, 0));
        assert_eq!(gif.fps, 10.0);
        assert!((gif.duration - 0.8).abs() < 1e-9, "{gif:?}");
        // The others hold 3 frames of 200 ms.
        for name in ["anim.webp", "anim.png"] {
            let info = probe(&ffprobe, &fixture(name)).unwrap();
            assert!(info.is_animation(), "{name}: {info:?}");
            assert_eq!((info.fps, info.stream), (5.0, 0), "{name}");
            assert!((info.duration - 0.6).abs() < 1e-9, "{name}: {info:?}");
        }
        // The AVIS lists its still item first and its sequence second.
        let avis = probe(&ffprobe, &fixture("anim.avif")).unwrap();
        assert!(avis.is_animation(), "{avis:?}");
        assert_eq!((avis.fps, avis.stream), (5.0, 1));
        assert!((avis.duration - 0.6).abs() < 1e-9, "{avis:?}");
    }

    #[test]
    fn non_video_fails_probe() {
        let Some(ffprobe) = real_ffprobe() else {
            return;
        };
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "not a video").unwrap();
        let error = probe(&ffprobe, file.path()).unwrap_err();
        assert!(error.to_string().contains("Invalid data"), "{error}");
        assert!(probe(&ffprobe, Path::new("/nonexistent/a.mov")).is_err());
        assert!(probe(Path::new("/nonexistent/ffprobe"), &fixture("anim.gif")).is_err());
    }

    /// A real video: `VIEWR_TEST_HDR_VIDEO` or `VIEWR_TEST_SDR_VIDEO`.
    #[test]
    #[ignore = "needs VIEWR_TEST_HDR_VIDEO or VIEWR_TEST_SDR_VIDEO, real video files"]
    fn real_videos_report_size_and_rate() {
        let Some(ffprobe) = real_ffprobe() else {
            return;
        };
        for variable in ["VIEWR_TEST_HDR_VIDEO", "VIEWR_TEST_SDR_VIDEO"] {
            let Some(path) = crate::decode::real_test_file(variable) else {
                continue;
            };
            let info = probe(&ffprobe, &path).unwrap();
            assert!(info.width > 0 && info.height > 0, "{info:?}");
            assert!(info.fps > 1.0 && info.duration > 0.0, "{info:?}");
            assert_eq!(info.hdr.is_some(), variable.contains("HDR"), "{info:?}");
        }
    }
}
