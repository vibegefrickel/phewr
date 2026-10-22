//! The facts about a video that the player needs before the first frame: the size after the
//! rotation, the frame rate, the duration, the HDR curve and the bit depth.
//!
//! One `ffprobe` process reads them from the headers and writes JSON. An animated image (GIF,
//! WebP, APNG, AVIS) gets its rate and its length from the packet durations, because the
//! containers tell no frame rate that fits the frame delays.

use super::tone_map::Hdr;
use lru::LruCache;
use serde::Deserialize;
use std::ffi::OsString;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// The header facts of a video. The size is the size on the screen, after the rotation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// The clockwise rotation that the display matrix asks for: 0, 90, 180 or 270 degrees.
    pub rotation: u16,
    pub fps: f64,
    /// The length in seconds, or 0 when the container does not tell it.
    pub duration: f64,
    pub hdr: Option<Hdr>,
    /// A 10 bit stream. The frames that the Mac hardware decoder gives back are `p010le` then.
    pub ten_bit: bool,
    /// An animated image. ffmpeg decodes it in software at the exact rate `fps`.
    pub animation: bool,
    /// A video with a variable frame rate. ffmpeg duplicates or drops frames to the constant
    /// rate `fps`, see [`variable_rate_fps`].
    pub variable_rate: bool,
    /// The index of the stream that ffmpeg maps. An AVIS file holds the still item as its
    /// first stream and the image sequence as its second.
    pub stream: usize,
}

impl VideoInfo {
    /// The frame interval at the frame rate of the video.
    pub(crate) fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps)
    }
}

/// The frame rate when the container gives none, or a rate that makes no sense.
const FALLBACK_FPS: f64 = 24.0;

/// The highest rate of an animated image. The player shows the frames at the rate of the
/// shortest common delay, so that each frame holds for its own delay. Above this rate, the
/// player takes the mean rate of the frames instead.
const MAX_ANIMATION_FPS: f64 = 50.0;

/// The highest output rate of a video with a variable frame rate. The terminal player shows
/// no more frames than this.
const MAX_VARIABLE_FPS: f64 = 60.0;

/// The share by which the average rate can differ from the real base rate of a constant-rate
/// video. iPhone clips have an average rate of 29.99 at a base rate of 30 (0.03 %).
const VARIABLE_RATE_TOLERANCE: f64 = 0.01;

/// How many packets the timing walk reads. The walk runs on the UI thread before the first
/// frame: 16 frames of a 181 MB GIF of 1280x720 pixels cost about 15 ms, all 300 cost 250 ms.
const TIMING_PACKETS: usize = 16;

/// How many video packets `ffprobe` reads for the timing walk of a file with one video
/// stream. One packet more than [`TIMING_PACKETS`] tells whether the walk reached the end.
const PACKET_LIMIT: usize = TIMING_PACKETS + 1;

/// The packet limit of an AVIF file. `-read_intervals` counts the packets of all video
/// streams, and an AVIS file interleaves its still item with its sequence.
const AVIS_PACKET_LIMIT: usize = 2 * PACKET_LIMIT;

/// The longest wait for `ffprobe`. A file on a stalled volume must not hold the UI thread in
/// `Player::open` or a thumbnail slot for longer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How many probes the session keeps, see [`ProbeCache`].
const PROBE_CACHE_ENTRIES: usize = 1024;

/// The fields of each video stream that the probe reads.
const STREAM_ENTRIES: &str = "stream=index,width,height,pix_fmt,color_transfer,r_frame_rate,\
    avg_frame_rate,time_base,nb_frames:stream_disposition=default,attached_pic:\
    stream_side_data=side_data_type,rotation:format=duration";

/// The JSON output of `ffprobe`. A field that `ffprobe` leaves out gets its default.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ProbeOutput {
    streams: Vec<Stream>,
    packets: Vec<Packet>,
    format: Format,
}

/// One video stream. `ffprobe` writes the rates and the frame count as text.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Stream {
    index: usize,
    width: u32,
    height: u32,
    pix_fmt: String,
    color_transfer: String,
    r_frame_rate: String,
    avg_frame_rate: String,
    time_base: String,
    nb_frames: String,
    disposition: Disposition,
    side_data_list: Vec<SideData>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Disposition {
    default: u8,
    attached_pic: u8,
}

/// The side data of a stream. A display matrix carries its counterclockwise `rotation`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SideData {
    side_data_type: String,
    rotation: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Format {
    duration: String,
}

/// One packet of the timing walk. The duration is in units of the time base of its stream.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Packet {
    stream_index: usize,
    duration: i64,
}

/// The packet durations of an animated image: the rate and, after a walk to the end, the length.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Timing {
    fps: f64,
    /// The sum of the durations, or `None` when the walk stopped before the end.
    duration: Option<f64>,
}

/// The packet limit of the timing walk of `path`, or `None` for a file that is no animation.
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

/// The `ffprobe` command line for `video`. Only an animation reads packets, at most
/// `packet_limit`.
pub(crate) fn probe_args(video: &Path, packet_limit: Option<usize>) -> Vec<OsString> {
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

/// Read the header of `path` with the `ffprobe` program. No frame is decoded.
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
    parse(&output.stdout, packet_limit)
}

/// The probe of `path` from the cache of the session, or a new probe that goes into it. The
/// thumbnail job probes each video of the grid, so `Player::open` then starts no `ffprobe`.
pub(crate) fn probe_cached(program: &Path, path: &Path) -> io::Result<VideoInfo> {
    static CACHE: LazyLock<ProbeCache> = LazyLock::new(|| ProbeCache::new(PROBE_CACHE_ENTRIES));
    CACHE.get_or_probe(path, || probe(program, path))
}

/// The probes of the session, keyed by path. Each entry keeps the mtime and the length of the
/// file, so a changed file probes again.
pub(crate) struct ProbeCache {
    entries: Mutex<LruCache<PathBuf, (FileStamp, VideoInfo)>>,
}

/// The mtime and the length of a file.
type FileStamp = (Option<SystemTime>, u64);

impl ProbeCache {
    /// A cache of at most `capacity` probes, so a session over many folders stays small.
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            entries: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// The cached probe of `path`, or the result of `run`. Only a success goes into the cache.
    pub(crate) fn get_or_probe(
        &self,
        path: &Path,
        run: impl FnOnce() -> io::Result<VideoInfo>,
    ) -> io::Result<VideoInfo> {
        let metadata = std::fs::metadata(path)?;
        let stamp = (metadata.modified().ok(), metadata.len());
        let cached = self
            .lock()
            .get(path)
            .filter(|(cached_stamp, _)| *cached_stamp == stamp)
            .map(|(_, info)| info.clone());
        if let Some(info) = cached {
            return Ok(info);
        }
        let info = run()?;
        self.lock().put(path.to_path_buf(), (stamp, info.clone()));
        Ok(info)
    }

    /// The entries. A thumbnail job that panicked leaves them usable.
    fn lock(&self) -> std::sync::MutexGuard<'_, LruCache<PathBuf, (FileStamp, VideoInfo)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Run `command` to its end and collect its stdout and stderr, as `Command::output` does. A
/// run longer than `timeout` is killed, and the error is `TimedOut`.
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
                reason = "a process that already ended cannot be killed, and that is fine"
            )]
            let _ = child.kill();
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the wait only reaps the process; its exit code does not matter"
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

/// The poll interval of [`output_within`]. A probe takes about 30 ms, so 1 ms adds little.
const OUTPUT_POLL: Duration = Duration::from_millis(1);

/// Read all of `pipe` on a thread, so that a full pipe never blocks the child.
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

/// An error for a file that is no video `ffprobe` can read.
fn invalid(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text.to_owned())
}

/// The header facts from the JSON of `ffprobe`, see [`probe_args`].
/// `packet_limit` is the limit of the timing walk of an animation, `None` for another file.
fn parse(json: &[u8], packet_limit: Option<usize>) -> io::Result<VideoInfo> {
    let animation = packet_limit.is_some();
    let output: ProbeOutput =
        serde_json::from_slice(json).map_err(|error| invalid(&error.to_string()))?;
    let stream =
        choose_stream(&output.streams, animation).ok_or_else(|| invalid("no video stream"))?;
    let (mut fps, variable_rate) = pacing_rate(
        fraction(&stream.r_frame_rate),
        fraction(&stream.avg_frame_rate),
        animation,
    );
    let mut duration = output
        .format
        .duration
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .unwrap_or(0.0);
    let rotation = stream
        .side_data_list
        .iter()
        .find(|data| data.side_data_type == "Display Matrix")
        .map_or(0, |data| clockwise_rotation(data.rotation));
    let (width, height) = if rotation % 180 == 0 {
        (stream.width, stream.height)
    } else {
        (stream.height, stream.width)
    };
    let hdr = Hdr::of_transfer(&stream.color_transfer);
    let ten_bit = stream.pix_fmt.contains("10") || stream.pix_fmt.contains("12");
    if let Some(limit) = packet_limit
        && let Some(timing) = animation_timing(&output.packets, stream, limit)
    {
        fps = timing.fps;
        if let Some(sum) = timing.duration {
            duration = sum;
        }
    }
    Ok(VideoInfo {
        width,
        height,
        rotation,
        fps,
        duration,
        hdr,
        ten_bit,
        animation,
        variable_rate,
        stream: stream.index,
    })
}

/// The value of a rate or a time base such as `30000/1001`, or `None` for `0/0` and no text.
fn fraction(text: &str) -> Option<f64> {
    let (numerator, denominator) = text.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    (numerator > 0.0 && denominator > 0.0).then(|| numerator / denominator)
}

/// The rate that the player paces at, and `true` when ffmpeg must get it as `-r`, from
/// `r_frame_rate` (`real`) and `avg_frame_rate` (`average`).
///
/// A video with a variable frame rate gets [`variable_rate_fps`]. Another video paces at its
/// average rate. Without an average rate, it paces at `real`, because ffmpeg writes raw frames
/// at `real` then. An animation gets its rate from the packets later, see [`timing_of`].
fn pacing_rate(real: Option<f64>, average: Option<f64>, animation: bool) -> (f64, bool) {
    let variable = (!animation)
        .then(|| variable_rate_fps(real?, average?))
        .flatten();
    match variable {
        Some(rate) => (rate, true),
        None => (average.or(real).map_or(FALLBACK_FPS, sanitize_fps), false),
    }
}

/// The constant output rate of a video with a variable frame rate, or `None` for a video with
/// a constant rate.
///
/// `real` is `r_frame_rate`, the rate of the shortest frame spacing that `FFmpeg` finds in
/// the first packets. `average` is the frame count over the length. A video is variable when
/// the two differ by more than [`VARIABLE_RATE_TOLERANCE`]. Without `-r`, ffmpeg writes raw
/// frames at `real`, but the player paced them at `average`: a screen recording at 25 and
/// 7.7 played 3.2 times too slow. The output rate is `real` up to [`MAX_VARIABLE_FPS`], so no
/// frame drops and the still parts repeat their frame. Above it, `real` is a fast burst (an
/// iPhone slow motion clip at 120 and 29.6) or a time base guess, and the output rate is
/// `average`, capped at [`MAX_VARIABLE_FPS`].
fn variable_rate_fps(real: f64, average: f64) -> Option<f64> {
    let valid = |rate: f64| rate.is_finite() && rate > 0.0;
    if !valid(real) || !valid(average) || (real - average).abs() <= real * VARIABLE_RATE_TOLERANCE {
        return None;
    }
    let rate = if real <= MAX_VARIABLE_FPS {
        real
    } else {
        average
    };
    Some(rate.min(MAX_VARIABLE_FPS))
}

/// The stream that the player shows. The player maps it with `-map`, so the frame size always
/// matches the output of ffmpeg. An animated file with more than one video stream gets the one
/// with the most frames: an AVIS file lists its still item first and its sequence second.
/// Another file gets the stream that ffmpeg would pick itself (`map_auto_video`): the area
/// plus 5,000,000 for the default flag, cover art last. On a tie, the first stream wins.
fn choose_stream(streams: &[Stream], animation: bool) -> Option<&Stream> {
    if animation && streams.len() >= 2 {
        return streams
            .iter()
            .max_by_key(|stream| stream.nb_frames.parse::<u64>().unwrap_or(0));
    }
    let score = |stream: &Stream| {
        if stream.disposition.attached_pic != 0 {
            return 1;
        }
        let area = u64::from(stream.width) * u64::from(stream.height);
        area + 5_000_000 * u64::from(stream.disposition.default != 0)
    };
    streams.iter().fold(None, |best, stream| match best {
        Some(best) if score(best) >= score(stream) => Some(best),
        _ => Some(stream),
    })
}

/// The rate from the first [`TIMING_PACKETS`] packets of `stream`, and the length when the
/// walk reached the end. `ffprobe` read at most `limit` packets. See [`timing_of`].
fn animation_timing(packets: &[Packet], stream: &Stream, limit: usize) -> Option<Timing> {
    let durations: Vec<i64> = packets
        .iter()
        .filter(|packet| packet.stream_index == stream.index)
        .map(|packet| packet.duration.max(0))
        .collect();
    let complete = packets.len() < limit && durations.len() <= TIMING_PACKETS;
    let walked = &durations[..durations.len().min(TIMING_PACKETS)];
    timing_of(walked, fraction(&stream.time_base)?, complete)
}

/// The rate and the length of the frame `durations`, in units of `seconds_per_unit`.
///
/// The rate is the inverse of the greatest common divisor of the delays, so a GIF with delays
/// of 100 ms and 500 ms plays at 10 frames per second, with each long frame sent 5 times.
/// Above [`MAX_ANIMATION_FPS`], the rate is the mean rate, so that each frame shows one time:
/// a GIF of 30 frames per second alternates delays of 30 and 40 ms (gcd 10 ms), and a cap
/// would round them to 20 and 40 ms. The length is the sum, when `complete`.
fn timing_of(durations: &[i64], seconds_per_unit: f64, complete: bool) -> Option<Timing> {
    let step = durations
        .iter()
        .fold(0, |step, &duration| gcd(step, duration));
    let total: i64 = durations.iter().sum();
    if step <= 0 || seconds_per_unit <= 0.0 {
        return None;
    }
    let step_fps = sanitize_fps(1.0 / (step as f64 * seconds_per_unit));
    let fps = if step_fps > MAX_ANIMATION_FPS {
        let mean = sanitize_fps(durations.len() as f64 / (total as f64 * seconds_per_unit));
        mean.min(MAX_ANIMATION_FPS)
    } else {
        step_fps
    };
    Some(Timing {
        fps,
        duration: complete.then_some(total as f64 * seconds_per_unit),
    })
}

/// The greatest common divisor of two durations. A zero does not change the other value.
fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A frame rate that the pacing can use.
fn sanitize_fps(fps: f64) -> f64 {
    if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        FALLBACK_FPS
    }
}

/// The clockwise rotation in whole quarter turns from the counterclockwise `degrees` of a
/// display matrix, as `ffprobe` writes it. iPhone portrait clips carry a rotation of -90, and
/// the player must turn them 90 degrees clockwise.
pub(crate) fn clockwise_rotation(degrees: f64) -> u16 {
    if !degrees.is_finite() {
        return 0;
    }
    let clockwise = (-degrees).rem_euclid(360.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is 0, 90, 180, 270 or 360"
    )]
    let quarter_turns = ((clockwise / 90.0).round() as u16) % 4;
    quarter_turns * 90
}

#[cfg(test)]
mod tests {
    use super::{
        ProbeCache, VideoInfo, clockwise_rotation, fraction, gcd, output_within, pacing_rate,
        packet_limit, parse, probe, probe_args, sanitize_fps, timing_of, variable_rate_fps,
    };
    use crate::player::tone_map::Hdr;
    use crate::test_support::fixture;
    use crate::test_support::video::real_ffprobe;
    use std::cell::Cell;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    /// The probe of an iPhone 15 clip: 4K HLG at 29.97 fps, average 29.99, without a rotation.
    const IPHONE_HLG: &str = r#"{"programs":[],"stream_groups":[],"streams":[{"index":0,"width":3840,"height":2160,"pix_fmt":"yuv420p10le","color_transfer":"arib-std-b67","r_frame_rate":"30000/1001","avg_frame_rate":"249600/8323","time_base":"1/600","nb_frames":"416","disposition":{"default":1,"attached_pic":0},"side_data_list":[{"side_data_type":"DOVI configuration record"},{"side_data_type":"Ambient viewing environment"}]}],"format":{"duration":"13.870000"}}"#;

    /// The probe of a 64x48 clip with a display rotation of -90 degrees, as an iPhone portrait
    /// clip carries it.
    const PORTRAIT: &str = r#"{"programs":[],"stream_groups":[],"streams":[{"index":0,"width":64,"height":48,"pix_fmt":"yuv420p","r_frame_rate":"10/1","avg_frame_rate":"10/1","time_base":"1/10240","nb_frames":"5","disposition":{"default":1,"attached_pic":0},"side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]}],"format":{"duration":"0.500000"}}"#;

    /// The probe of the variable-rate clip of `write_clip`: 20 frames at 60 and 20 at 15 fps.
    const VARIABLE: &str = r#"{"programs":[],"stream_groups":[],"streams":[{"index":0,"width":160,"height":120,"pix_fmt":"yuv420p","r_frame_rate":"60/1","avg_frame_rate":"2400/97","time_base":"1/15360","nb_frames":"40","disposition":{"default":1,"attached_pic":0}}],"format":{"duration":"1.616667"}}"#;

    /// The probe of `anim.gif` with its packets: delays of 100, 500 and 200 ms.
    const GIF: &str = r#"{"packets":[{"stream_index":0,"duration":10},{"stream_index":0,"duration":50},{"stream_index":0,"duration":20}],"programs":[],"stream_groups":[],"streams":[{"index":0,"width":16,"height":12,"pix_fmt":"bgra","r_frame_rate":"6/1","avg_frame_rate":"50/13","time_base":"1/100","nb_frames":"3","disposition":{"default":0,"attached_pic":0}}],"format":{"duration":"0.800000"}}"#;

    /// The probe of `anim.avif` with its packets: the still item, then 3 frames of 200 ms.
    const AVIS: &str = r#"{"packets":[{"stream_index":0,"duration":1},{"stream_index":1,"duration":1},{"stream_index":1,"duration":1},{"stream_index":1,"duration":1}],"programs":[],"stream_groups":[],"streams":[{"index":0,"width":16,"height":12,"pix_fmt":"yuv444p","color_transfer":"iec61966-2-1","r_frame_rate":"1/1","avg_frame_rate":"1/1","time_base":"1/1","nb_frames":"1","disposition":{"default":1,"attached_pic":0}},{"index":1,"width":16,"height":12,"pix_fmt":"yuv444p","color_transfer":"iec61966-2-1","r_frame_rate":"5/1","avg_frame_rate":"5/1","time_base":"1/5","nb_frames":"3","disposition":{"default":1,"attached_pic":0}}],"format":{"duration":"0.600000"}}"#;

    /// The probe of a GIF of 40 frames of 40 ms, cut after the packet limit of 17.
    fn long_gif() -> String {
        let packets = vec![r#"{"stream_index":0,"duration":4}"#; 17].join(",");
        format!(
            r#"{{"packets":[{packets}],"streams":[{{"index":0,"width":32,"height":24,"pix_fmt":"bgra","r_frame_rate":"25/1","avg_frame_rate":"25/1","time_base":"1/100","nb_frames":"40","disposition":{{"default":0,"attached_pic":0}}}}],"format":{{"duration":"1.600000"}}}}"#
        )
    }

    #[test]
    fn an_iphone_hlg_clip_parses_as_constant_rate_10_bit_hlg() {
        let info = parse(IPHONE_HLG.as_bytes(), None).unwrap();
        assert_eq!((info.width, info.height, info.rotation), (3840, 2160, 0));
        assert_eq!(info.hdr, Some(Hdr::Hlg));
        assert!(
            info.ten_bit && !info.variable_rate && !info.animation,
            "{info:?}"
        );
        assert!(
            (info.fps - 249_600.0 / 8323.0).abs() < 1e-9,
            "the average rate"
        );
        assert!((info.duration - 13.87).abs() < 1e-9);
        assert_eq!(info.stream, 0);
    }

    #[test]
    fn a_portrait_clip_parses_with_its_turned_size() {
        let info = parse(PORTRAIT.as_bytes(), None).unwrap();
        assert_eq!((info.width, info.height, info.rotation), (48, 64, 90));
        assert_eq!(info.hdr, None);
        assert!(!info.ten_bit);
        assert_eq!(info.fps, 10.0);
    }

    #[test]
    fn a_variable_rate_clip_parses_as_variable_at_its_base_rate() {
        let info = parse(VARIABLE.as_bytes(), None).unwrap();
        assert!(info.variable_rate, "{info:?}");
        assert_eq!(info.fps, 60.0);
        assert!((info.duration - 1.616_667).abs() < 1e-9);
    }

    #[test]
    fn animations_parse_their_rate_and_length_from_the_packets() {
        let gif = parse(GIF.as_bytes(), Some(17)).unwrap();
        assert!(gif.animation, "{gif:?}");
        assert_eq!((gif.width, gif.height, gif.stream), (16, 12, 0));
        assert_eq!(gif.fps, 10.0);
        assert!((gif.duration - 0.8).abs() < 1e-9, "{gif:?}");
        let avis = parse(AVIS.as_bytes(), Some(34)).unwrap();
        assert_eq!(
            (avis.fps, avis.stream),
            (5.0, 1),
            "the sequence, not the still item"
        );
        assert!((avis.duration - 0.6).abs() < 1e-9, "{avis:?}");
        // The cut walk keeps the length of the container.
        let long = parse(long_gif().as_bytes(), Some(17)).unwrap();
        assert_eq!(long.fps, 25.0);
        assert!((long.duration - 1.6).abs() < 1e-9, "{long:?}");
        // Without the animation extension, the packets do not count, and the container rates
        // of 6 and 3.85 make a variable rate.
        let plain = parse(GIF.as_bytes(), None).unwrap();
        assert!(plain.variable_rate && !plain.animation, "{plain:?}");
        assert_eq!(plain.fps, 6.0);
    }

    #[test]
    fn the_stream_gets_the_score_of_ffmpeg() {
        let json = r#"{"streams":[
            {"index":0,"width":600,"height":600,"disposition":{"default":0,"attached_pic":1}},
            {"index":1,"width":320,"height":240,"disposition":{"default":0,"attached_pic":0}},
            {"index":2,"width":160,"height":120,"disposition":{"default":1,"attached_pic":0}},
            {"index":3,"width":160,"height":120,"disposition":{"default":1,"attached_pic":0}}
        ]}"#;
        assert_eq!(parse(json.as_bytes(), None).unwrap().stream, 2);
        // ffmpeg adds 5,000,000 for the default flag: a 4K stream beats a default 1080p one.
        let uhd = r#"{"streams":[
            {"index":0,"width":1920,"height":1080,"disposition":{"default":1,"attached_pic":0}},
            {"index":1,"width":3840,"height":2160,"disposition":{"default":0,"attached_pic":0}}
        ]}"#;
        assert_eq!(parse(uhd.as_bytes(), None).unwrap().stream, 1);
        let cover_only = r#"{"streams":[{"index":4,"width":600,"height":600,
            "disposition":{"default":0,"attached_pic":1}}]}"#;
        assert_eq!(parse(cover_only.as_bytes(), None).unwrap().stream, 4);
    }

    #[test]
    fn no_stream_or_no_json_is_an_error() {
        for (json, text) in [
            ("{}", "no video stream"),
            (r#"{"streams":[]}"#, "no video stream"),
        ] {
            assert_eq!(parse(json.as_bytes(), None).unwrap_err().to_string(), text);
        }
        assert!(parse(b"not json", None).is_err());
        let bare = parse(br#"{"streams":[{"index":0}]}"#, None).unwrap();
        assert_eq!((bare.fps, bare.duration), (24.0, 0.0), "the fallbacks");
    }

    #[test]
    fn rates_and_time_bases_parse_as_fractions() {
        assert_eq!(fraction("30/1"), Some(30.0));
        assert_eq!(fraction("1/100"), Some(0.01));
        assert_eq!(fraction("0/0"), None);
        assert_eq!(fraction("25"), None);
        assert_eq!(fraction(""), None);
        assert_eq!(fraction("-1/2"), None);
    }

    #[test]
    fn only_an_animation_reads_packets() {
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
    fn only_an_avif_file_reads_packets_for_two_streams() {
        assert_eq!(packet_limit(Path::new("clip.mov")), None);
        assert_eq!(packet_limit(Path::new("still.jpg")), None);
        for name in ["a.gif", "b.webp", "c.png", "d.APNG"] {
            assert_eq!(packet_limit(Path::new(name)), Some(17), "{name}");
        }
        assert_eq!(packet_limit(Path::new("e.avif")), Some(34));
        assert_eq!(packet_limit(Path::new("f.AVIF")), Some(34));
    }

    #[test]
    fn the_display_rotation_gives_the_clockwise_rotation() {
        assert_eq!(clockwise_rotation(0.0), 0);
        assert_eq!(clockwise_rotation(-90.0), 90, "an iPhone portrait clip");
        assert_eq!(clockwise_rotation(90.0), 270);
        assert_eq!(clockwise_rotation(180.0), 180);
        assert_eq!(clockwise_rotation(-180.0), 180);
        assert_eq!(clockwise_rotation(-89.6), 90, "rounded to a quarter turn");
        assert_eq!(clockwise_rotation(f64::NAN), 0);
    }
    #[test]
    fn invalid_frame_rates_use_the_fallback() {
        for fps in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert_eq!(sanitize_fps(fps), 24.0);
        }
        assert_eq!(sanitize_fps(29.97), 29.97);
        let info = VideoInfo {
            width: 16,
            height: 9,
            rotation: 0,
            fps: 25.0,
            duration: 1.0,
            hdr: None,
            ten_bit: false,
            animation: false,
            variable_rate: false,
            stream: 0,
        };
        assert_eq!(info.frame_interval(), Duration::from_millis(40));
    }

    #[test]
    fn a_variable_rate_plays_at_its_base_rate_up_to_60_and_else_at_its_average() {
        assert_eq!(variable_rate_fps(30.0, 30.0), None, "a constant rate");
        assert_eq!(
            variable_rate_fps(30.0, 100_080.0 / 3337.0),
            None,
            "an iPhone clip at 29.99 is constant"
        );
        assert_eq!(
            variable_rate_fps(25.0, 7.7),
            Some(25.0),
            "a screen recording"
        );
        assert_eq!(variable_rate_fps(60.0, 24.0), Some(60.0));
        assert_eq!(
            variable_rate_fps(120.0, 29.6),
            Some(29.6),
            "an iPhone slow motion clip"
        );
        assert_eq!(
            variable_rate_fps(90_000.0, 75.0),
            Some(60.0),
            "a time base guess, capped"
        );
        for (real, average) in [
            (0.0, 24.0),
            (24.0, 0.0),
            (f64::NAN, 24.0),
            (24.0, f64::INFINITY),
        ] {
            assert_eq!(variable_rate_fps(real, average), None);
        }
    }

    #[test]
    fn the_pacing_rate_follows_the_rate_that_ffmpeg_writes() {
        assert_eq!(pacing_rate(Some(30.0), Some(30.0), false), (30.0, false));
        assert_eq!(pacing_rate(Some(25.0), Some(7.7), false), (25.0, true));
        assert_eq!(
            pacing_rate(Some(25.0), Some(7.7), true),
            (7.7, false),
            "an animation gets no variable rate"
        );
        assert_eq!(
            pacing_rate(Some(30.0), None, false),
            (30.0, false),
            "without an average rate, ffmpeg writes at the real rate"
        );
        assert_eq!(pacing_rate(None, Some(25.0), false), (25.0, false));
        assert_eq!(pacing_rate(None, None, false), (24.0, false));
    }

    #[test]
    fn the_cache_probes_a_file_once_until_it_changes() {
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

    #[test]
    fn a_program_that_hangs_is_killed_after_the_timeout() {
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

    /// Probe a variable and a constant-rate clip that the real ffmpeg writes. Without ffmpeg
    /// or ffprobe on PATH, the test skips with a note.
    #[test]
    fn a_variable_rate_clip_probes_as_variable_and_a_constant_one_does_not() {
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
            assert_eq!(info.variable_rate, variable, "{name}: {info:?}");
            let fps = if variable { 60.0 } else { 24.0 };
            assert!((info.fps - fps).abs() < 1e-9, "{name}: {info:?}");
            assert!(!info.animation, "{name}");
        }
    }

    #[test]
    fn a_fast_animation_plays_at_its_mean_rate_and_a_cut_walk_has_no_length() {
        // Delays of 30 and 40 ms in 10 ms units: gcd 10 ms is 100 fps, above the cap.
        let fast = timing_of(&[3, 4, 3, 4, 3, 4], 0.01, true).unwrap();
        assert!((fast.fps - 6.0 / 0.21).abs() < 1e-9, "{fast:?}");
        assert!((fast.duration.unwrap() - 0.21).abs() < 1e-9);
        // Delays of 100 and 500 ms: the gcd rate of 10 fps.
        let slow = timing_of(&[10, 50, 20], 0.01, false).unwrap();
        assert_eq!(slow.fps, 10.0);
        assert_eq!(slow.duration, None, "the walk stopped before the end");
        // A mean above the cap stays at the cap. No duration gives no timing.
        assert_eq!(timing_of(&[1, 1, 1], 0.001, true).unwrap().fps, 50.0);
        assert_eq!(timing_of(&[0, 0], 0.01, true), None);
        assert_eq!(timing_of(&[], 0.01, true), None);
    }

    #[test]
    fn the_shortest_common_delay_gives_the_rate() {
        assert_eq!(gcd(10, 50), 10);
        assert_eq!(gcd(0, 20), 20);
        assert_eq!(gcd(20, 0), 20);
        assert_eq!(gcd(3, 5), 1);
    }

    /// The animated fixtures: 3 frames of 16x12 pixels each. Without ffprobe on PATH, the test
    /// skips with a note.
    #[test]
    fn animated_images_report_their_rate_and_length_from_the_packets() {
        let Some(ffprobe) = real_ffprobe() else {
            return;
        };
        // The GIF has delays of 100, 500 and 200 ms: the rate is 10 and the length 0.8.
        let gif = probe(&ffprobe, &fixture("anim.gif")).unwrap();
        assert!(gif.animation, "{gif:?}");
        assert_eq!((gif.width, gif.height, gif.stream), (16, 12, 0));
        assert_eq!(gif.fps, 10.0);
        assert!((gif.duration - 0.8).abs() < 1e-9, "{gif:?}");
        // The others hold 3 frames of 200 ms.
        for name in ["anim.webp", "anim.png"] {
            let info = probe(&ffprobe, &fixture(name)).unwrap();
            assert!(info.animation, "{name}: {info:?}");
            assert_eq!((info.fps, info.stream), (5.0, 0), "{name}");
            assert!((info.duration - 0.6).abs() < 1e-9, "{name}: {info:?}");
        }
        // The AVIS lists its still item first; the sequence is the second stream.
        let avis = probe(&ffprobe, &fixture("anim.avif")).unwrap();
        assert!(avis.animation, "{avis:?}");
        assert_eq!((avis.fps, avis.stream), (5.0, 1));
        assert!((avis.duration - 0.6).abs() < 1e-9, "{avis:?}");
    }

    #[test]
    fn a_file_that_is_no_video_does_not_probe() {
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

    /// Read the header of a real video: `VIEWR_TEST_HDR_VIDEO` or `VIEWR_TEST_SDR_VIDEO`.
    #[test]
    #[ignore = "needs VIEWR_TEST_HDR_VIDEO or VIEWR_TEST_SDR_VIDEO, real video files"]
    fn real_videos_report_their_size_and_rate() {
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
