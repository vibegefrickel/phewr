//! Tests of the pacing, the restart and the fallback of the terminal player.

use super::{DECODE_POLL, Frame, IDLE_POLL, Player, VideoInfo};
use crate::test_support::video::{FakeDecoder, fake_decoder, info_of, shared};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(100);

/// A 64x48 clip at 10 fps in a 64x48 frame box.
fn player(decoder: &Rc<FakeDecoder>, repeat: bool) -> Player {
    Player::open(
        shared(decoder),
        Path::new("/bin/ffmpeg"),
        Path::new("/videos/clip.mp4"),
        (64, 48),
        &[],
        repeat,
    )
    .unwrap()
}

fn frame(value: u8) -> Vec<u8> {
    vec![value; 64 * 48 * 3]
}

#[test]
fn open_starts_ffmpeg_at_frame_box() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let player = player(&decoder, true);
    assert_eq!(decoder.starts().len(), 1);
    let args = decoder.last_args();
    assert!(args.contains("scale_vt=w=64:h=48,"), "{args}");
    assert!(args.contains(" -i /videos/clip.mp4 "), "{args}");
    assert!(args.ends_with(" -pix_fmt rgb24 pipe:1"), "{args}");
    assert!(player.playing());
    assert_eq!(player.position(), 0.0);
    assert_eq!(player.duration(), 2.0);
    assert_eq!(
        player.time_until_next_frame_at(Instant::now()),
        DECODE_POLL,
        "no frame yet: a short poll"
    );
}

#[test]
fn frameless_run_ends_after_deadline() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut player = player(&decoder, false);
    let clock = Instant::now();
    assert!(!player.poll_at(clock + Duration::from_secs(4)));
    assert!(player.playing(), "within the deadline");
    // The GPU run falls back to software and gets its own deadline.
    assert!(!player.poll_at(clock + Duration::from_secs(6)));
    assert_eq!(decoder.starts().len(), 2);
    assert!(player.playing());
    assert!(!player.poll_at(clock + Duration::from_secs(12)));
    assert!(player.finished());
    assert!(player.error.is_some());
}

#[test]
fn first_frame_at_4_seconds_plays() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut player = player(&decoder, false);
    let clock = Instant::now();
    assert!(!player.poll_at(clock + Duration::from_secs(4)));
    for value in 0..25 {
        decoder.feed(frame(value));
    }
    let first = clock + Duration::from_secs(4);
    assert!(player.poll_at(first));
    // The deadline is for the first frame only.
    for step in 1..=21 {
        player.poll_at(first + FRAME * step);
    }
    assert!(player.playing());
    assert!(player.error.is_none());
    assert_eq!(decoder.starts().len(), 1);
}

#[test]
fn frame_cuts_tail_rejects_short_data() {
    let frame = Frame::new((4, 2), false, vec![5; 4 * 2 * 3 + 1]).unwrap();
    let pixels = frame.pixels();
    assert_eq!(frame.dimensions(), (4, 2));
    assert!(!pixels.alpha);
    assert_eq!(pixels.data, [5; 4 * 2 * 3], "the tail is cut");
    assert!(Frame::new((4, 2), true, vec![5; 4 * 2 * 3]).is_none());
}

#[test]
fn short_frame_keeps_last_frame() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut player = player(&decoder, true);
    let clock = Instant::now();
    decoder.feed(frame(1));
    assert!(player.poll_at(clock));
    decoder.feed(vec![2; 10]);
    assert!(!player.poll_at(clock + FRAME), "no new frame");
    assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 1);
    assert_eq!(player.dropped, 1);
    decoder.feed(frame(3));
    assert!(player.poll_at(clock + FRAME * 2));
    assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 3);
}

#[test]
fn late_frame_drops_for_newer_one() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut player = player(&decoder, true);
    let clock = Instant::now();
    assert!(!player.poll_at(clock), "no frame from ffmpeg yet");
    decoder.feed(frame(1));
    decoder.feed(frame(2));
    assert!(player.poll_at(clock), "the first frame starts the clock");
    assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 1);
    assert!(
        !player.poll_at(clock + FRAME / 2),
        "the second frame is not due"
    );
    assert_eq!(
        player.time_until_next_frame_at(clock + FRAME / 2),
        FRAME / 2
    );
    assert!(player.poll_at(clock + FRAME));
    assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 2);
    assert!((player.position() - 0.2).abs() < 1e-9);

    // Frame 3 is due at 200 ms. At 350 ms it is more than one interval late and drops.
    decoder.feed(frame(3));
    decoder.feed(frame(4));
    assert!(
        player.poll_at(clock + FRAME * 7 / 2),
        "frame 4 shows instead"
    );
    assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 4);
    assert_eq!(player.dropped, 1);
    assert!((player.position() - 0.4).abs() < 1e-9);
}

#[test]
fn slow_decode_keeps_clock() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut player = player(&decoder, true);
    let clock = Instant::now();
    decoder.feed(frame(1));
    assert!(player.poll_at(clock));
    decoder.feed(frame(2));
    assert!(player.poll_at(clock + FRAME));
    // Frame 3 is due at 200 ms and comes at 250 ms, with no newer frame: it shows.
    decoder.feed(frame(3));
    assert!(
        player.poll_at(clock + FRAME * 5 / 2),
        "the late frame shows"
    );
    assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 3);
    assert_eq!(player.dropped, 0);
    // The clock stays: frame 4 is due at 300 ms, not one interval after the late frame.
    decoder.feed(frame(4));
    assert!(!player.poll_at(clock + FRAME * 29 / 10));
    assert_eq!(
        player.time_until_next_frame_at(clock + FRAME * 5 / 2),
        FRAME / 2
    );
    assert!(player.poll_at(clock + FRAME * 3));
    assert_eq!(
        decoder.starts().len(),
        1,
        "a small lag starts no new ffmpeg"
    );
}

#[test]
fn restart_past_stream_end_ends_quietly() {
    for repeat in [false, true] {
        let mut info = info_of(640, 480, 10.0);
        info.duration = 20.0;
        let decoder = fake_decoder(info);
        let mut player = player(&decoder, repeat);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        // 1 s without a frame: ffmpeg starts again ahead of the clock.
        assert!(!player.poll_at(clock + FRAME * 10));
        assert_eq!(decoder.starts().len(), 2);
        // The stream is shorter than the container says: the new run has no frame.
        decoder.end();
        assert!(!player.poll_at(clock + FRAME * 11));
        assert_eq!(player.error, None, "repeat {repeat}");
        assert_eq!(player.finished(), !repeat, "repeat {repeat}");
        if repeat {
            assert_eq!(decoder.starts().len(), 3, "the clip starts again");
            assert!(!decoder.last_args().contains("-ss"));
        } else {
            assert_eq!(decoder.starts().len(), 2, "no retry");
        }
    }
}

#[test]
fn lag_restarts_ahead_of_clock() {
    let mut info = info_of(640, 480, 10.0);
    info.duration = 20.0;
    let decoder = fake_decoder(info);
    let mut player = player(&decoder, true);
    let clock = Instant::now();
    decoder.feed(frame(1));
    assert!(player.poll_at(clock));
    // Frame 2 is due at 100 ms. At 400 ms it lags by exactly the limit: no restart yet.
    assert!(!player.poll_at(clock + FRAME * 4));
    assert_eq!(decoder.starts().len(), 1);
    // At 500 ms the lag passes the limit: ffmpeg starts at the clock plus the lead.
    assert!(!player.poll_at(clock + FRAME * 5));
    assert_eq!(decoder.starts().len(), 2);
    assert!(
        decoder.last_args().contains(" -ss 0.800 -i "),
        "{}",
        decoder.last_args()
    );
    assert_eq!(player.dropped, 7, "the frames from 100 ms to 700 ms");
    // The new run keeps the clock: its first frame is due at 800 ms.
    decoder.feed(frame(9));
    assert!(!player.poll_at(clock + FRAME * 5));
    assert_eq!(
        player.time_until_next_frame_at(clock + FRAME * 5),
        FRAME * 3
    );
    assert!(!player.poll_at(clock + FRAME * 7));
    assert!(player.poll_at(clock + FRAME * 8));
    assert!((player.position() - 0.9).abs() < 1e-9);
    // Close to the end, the run plays out and starts nothing.
    let mut info = info_of(640, 480, 10.0);
    info.duration = 1.0;
    let short_decoder = fake_decoder(info);
    let mut short = self::player(&short_decoder, true);
    short_decoder.feed(frame(1));
    assert!(short.poll_at(clock));
    assert!(!short.poll_at(clock + FRAME * 5));
    assert_eq!(short_decoder.starts().len(), 1);
}

#[test]
fn unknown_length_has_no_end_margin() {
    let mut info = info_of(640, 480, 10.0);
    info.duration = 0.0;
    let decoder = fake_decoder(info);
    let mut player = player(&decoder, true);
    assert_eq!(player.duration(), 0.0);
    let clock = Instant::now();
    decoder.feed(frame(1));
    assert!(player.poll_at(clock));
    // A lag past the limit restarts ffmpeg, and no clip length ends the run or stops it.
    assert!(!player.poll_at(clock + FRAME * 5));
    assert_eq!(decoder.starts().len(), 2);
    assert!(player.playing(), "the unknown length ends nothing");
    assert!(
        decoder.last_args().contains(" -ss 0.800 -i "),
        "{}",
        decoder.last_args()
    );
}

/// Each frame write takes 3 intervals, and ffmpeg keeps two frames ready.
#[test]
fn slow_terminal_drops_frames() {
    const CLIP_FRAMES: u32 = 60;
    let mut info = info_of(640, 480, 10.0);
    info.duration = 6.0;
    let decoder = fake_decoder(info);
    let mut player = player(&decoder, false);
    let clock = Instant::now();
    let mut now = clock;
    let (mut runs, mut fed, mut shown) = (1, 0, 0);
    while !player.finished() && now < clock + FRAME * 200 {
        if decoder.starts().len() > runs {
            (runs, fed) = (decoder.starts().len(), 0);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the start of a run of the 6 s test clip, in frames"
        )]
        let first = (player.run.base * 10.0).round() as u32;
        let taken = player.run.due_frames + u32::from(player.pending.is_some());
        while fed < taken + 2 && first + fed < CLIP_FRAMES {
            decoder.feed(frame(1));
            fed += 1;
        }
        if first + fed >= CLIP_FRAMES && taken >= fed {
            decoder.end();
        }
        if player.poll_at(now) {
            shown += 1;
            now += FRAME * 3;
        } else {
            now += FRAME / 10;
        }
    }
    assert!(player.finished());
    let played = now.duration_since(clock);
    // The last frame shows at most one slow write after the 6 s: 6.6 s at the most.
    assert!(
        played <= FRAME * 66,
        "the 6 s clip ended after {played:?}, not in slow motion"
    );
    assert!(player.dropped > 0, "frames dropped instead");
    assert!(shown >= 15, "{shown} frames shown");
}

#[test]
fn animation_decodes_in_software() {
    use crate::test_support::video::animation_info;
    let decoder = fake_decoder(animation_info());
    let mut player = player(&decoder, false);
    let args = decoder.last_args();
    assert!(
        args.contains(" -map 0:0 -fps_mode cfr -r 10.0000 "),
        "{args}"
    );
    assert!(args.contains("scale=w=64:h=48:flags=bilinear"), "{args}");
    assert!(!args.contains("-hwaccel"), "{args}");
    assert!(args.ends_with(" -pix_fmt rgba pipe:1"), "{args}");

    player.play_at_least(Duration::from_millis(300));
    let clock = Instant::now();
    decoder.feed(vec![1; 64 * 48 * 4]);
    assert!(player.poll_at(clock));
    let pixels = player.current_frame.as_ref().unwrap().pixels();
    assert!(pixels.alpha, "an animation keeps its transparent pixels");
    assert_eq!(pixels.data.len(), 64 * 48 * 4);
    decoder.end();
    assert!(!player.poll_at(clock + FRAME));
    assert!(
        player.playing(),
        "the loop starts again before the hold passed"
    );
    assert_eq!(decoder.starts().len(), 2);
    assert!(
        !decoder.last_args().contains("hwaccel_output_format"),
        "no GPU retry for an animation"
    );

    decoder.feed(vec![2; 64 * 48 * 4]);
    assert!(player.poll_at(clock + 2 * FRAME));
    decoder.end();
    assert!(!player.poll_at(clock + Duration::from_millis(400)));
    assert!(
        player.finished(),
        "the end of the loop after the hold ends the playback"
    );
    assert_eq!(decoder.starts().len(), 2);
    assert_eq!(player.error, None);
}

#[test]
fn variable_rate_paces_at_output_rate() {
    let decoder = fake_decoder(VideoInfo {
        rate: super::Rate::Variable,
        ..info_of(640, 480, 20.0)
    });
    let mut player = player(&decoder, true);
    let args = decoder.last_args();
    assert!(
        args.contains(" -i /videos/clip.mp4 -map 0:0 -fps_mode cfr -r 20.0000 "),
        "{args}"
    );
    assert!(args.contains("-hwaccel videotoolbox"), "{args}");
    let step = Duration::from_millis(50);
    let clock = Instant::now();
    for value in 1..=10 {
        decoder.feed(frame(value));
        let due = clock + step * u32::from(value - 1);
        if value > 1 {
            assert!(
                !player.poll_at(due.checked_sub(step / 2).unwrap()),
                "frame {value} is not due"
            );
        }
        assert!(player.poll_at(due), "frame {value}");
    }
    assert_eq!(player.dropped, 0);
    assert!((player.position() - 0.5).abs() < 1e-9);
}

/// The name, the player at the end, the frames that ffmpeg wrote and the played seconds.
/// Empty without ffmpeg or libx264.
fn play_real_clips(real_clock: bool) -> Vec<(&'static str, Player, u32, f64)> {
    use crate::test_support::video::{real_ffmpeg, real_ffprobe, write_clip};
    let (Some(program), Some(ffprobe)) = (real_ffmpeg(), real_ffprobe()) else {
        return Vec::new();
    };
    let dir = tempfile::tempdir().unwrap();
    let mut played = Vec::new();
    for (name, variable) in [
        ("vfr.mp4", true),
        ("vfr.mov", true),
        ("cfr.mp4", false),
        ("cfr.mov", false),
    ] {
        let Some(path) = write_clip(&program, &dir.path().join(name), variable) else {
            return Vec::new();
        };
        let (player, frames, seconds) = play_to_end(&program, &ffprobe, &path, real_clock);
        println!(
            "{name}: {frames} frames at {:.2} fps ({} dropped): played {seconds:.3} s, \
             container {:.3} s",
            player.info.fps,
            player.dropped,
            player.duration()
        );
        played.push((name, player, frames, seconds));
    }
    played
}

/// Play `path` to its end. Without `real_clock`, a virtual clock jumps to each due frame and
/// the scheduler cannot change the result.
fn play_to_end(
    program: &Path,
    ffprobe: &Path,
    path: &Path,
    real_clock: bool,
) -> (Player, u32, f64) {
    let decoder: Rc<dyn super::Decoder> =
        Rc::new(super::SystemDecoder::new(Some(ffprobe.to_path_buf())));
    let mut player = Player::open(decoder, program, path, (32, 24), &[], false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut clock = Instant::now();
    let (mut first, mut last, mut shown) = (None, None, 0_u32);
    while !player.finished() {
        assert!(
            Instant::now() < deadline,
            "{}: the player hangs",
            path.display()
        );
        if real_clock {
            clock = Instant::now();
        }
        if player.poll_at(clock) {
            first.get_or_insert(clock);
            last = Some(clock);
            shown += 1;
        } else if !real_clock && player.pending.is_some() {
            clock += player.time_until_next_frame_at(clock);
        } else {
            let wait = player.time_until_next_frame_at(clock);
            std::thread::sleep(wait.min(Duration::from_millis(2)));
        }
    }
    assert_eq!(player.error, None, "{}", path.display());
    let (Some(first), Some(last)) = (first, last) else {
        panic!("{}: no frame", path.display());
    };
    let seconds = (last - first + player.interval).as_secs_f64();
    let frames = shown + player.dropped;
    (player, frames, seconds)
}

/// A virtual clock: the frames times the interval match the container length.
#[test]
fn real_clips_play_at_their_length() {
    for (name, player, frames, seconds) in play_real_clips(false) {
        let paced = (player.interval * frames).as_secs_f64();
        let length = player.duration();
        assert!(
            (paced - length).abs() < 0.1 * length,
            "{name}: paced {paced}"
        );
        assert!((seconds - paced).abs() < 1e-6, "{name}: played {seconds}");
        assert_eq!(player.dropped, 0, "{name}: the virtual clock is never late");
    }
}

/// The wall time matches the container length. It takes about 7 s and depends on the
/// scheduler.
#[test]
#[ignore = "plays 4 clips at the real clock for about 7 s"]
fn real_clips_play_in_wall_time() {
    for (name, player, _, seconds) in play_real_clips(true) {
        let length = player.duration();
        assert!(
            (seconds - length).abs() < 0.25 * length,
            "{name}: wall {seconds}"
        );
    }
}

#[test]
fn clip_end_repeats_or_stops() {
    for repeat in [true, false] {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, repeat);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        decoder.end();
        assert!(!player.poll_at(clock + FRAME));
        assert_eq!(player.finished(), !repeat, "repeat {repeat}");
        assert_eq!(decoder.starts().len(), if repeat { 2 } else { 1 });
        if repeat {
            assert!(player.playing());
            decoder.feed(frame(2));
            assert!(player.poll_at(clock + FRAME * 2));
            assert!(
                (player.position() - 0.1).abs() < 1e-9,
                "the position starts at 0 again"
            );
        }
    }
}

#[test]
fn frameless_run_retries_in_software() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut player = player(&decoder, true);
    let clock = Instant::now();
    decoder.end();
    assert!(!player.poll_at(clock));
    assert_eq!(decoder.starts().len(), 2);
    let args = decoder.last_args();
    assert!(!args.contains("videotoolbox_vld"), "{args}");
    assert!(args.contains("scale=w=64:h=48:flags=bilinear"), "{args}");
    assert!(player.playing() && !player.finished());
    decoder.end();
    assert!(!player.poll_at(clock));
    assert!(player.finished());
    assert_eq!(
        player.error.as_deref(),
        Some("ffmpeg could not decode clip.mp4")
    );
    assert_eq!(decoder.starts().len(), 2, "no third try");
    assert_eq!(player.time_until_next_frame_at(clock), IDLE_POLL);
}

#[test]
fn gpu_failure_is_paid_once() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut first = player(&decoder, true);
    let clock = Instant::now();
    decoder.end();
    assert!(!first.poll_at(clock));
    assert_eq!(decoder.remembered(), 0, "no software frame yet");
    decoder.feed(frame(1));
    assert!(first.poll_at(clock));
    assert_eq!(decoder.remembered(), 1);
    decoder.feed(frame(2));
    assert!(first.poll_at(clock + FRAME));
    assert_eq!(decoder.remembered(), 1, "marked once");
    let _second = player(&decoder, true);
    let args = decoder.last_args();
    assert!(!args.contains("videotoolbox_vld"), "{args}");
    assert_eq!(decoder.starts().len(), 3, "the second start skips the GPU");
}

#[test]
fn software_failure_or_timeout_marks_nothing() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut failing = player(&decoder, true);
    let clock = Instant::now();
    decoder.end();
    assert!(!failing.poll_at(clock));
    decoder.end();
    assert!(!failing.poll_at(clock));
    assert!(failing.finished());
    assert_eq!(decoder.remembered(), 0, "both runs failed");

    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut slow = player(&decoder, true);
    let late = clock + Duration::from_secs(30);
    assert!(!slow.poll_at(late));
    assert_eq!(decoder.starts().len(), 2, "the deadline starts software");
    decoder.feed(frame(1));
    assert!(slow.poll_at(late));
    assert_eq!(decoder.remembered(), 0, "a timeout is no GPU failure");

    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let mut cancelled = player(&decoder, true);
    decoder.end();
    assert!(!cancelled.poll_at(clock));
    drop(cancelled);
    assert_eq!(decoder.remembered(), 0, "a dropped player marks nothing");
}

#[test]
fn failed_probe_or_start_fails_open() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    decoder.fail_probe.set(true);
    assert!(
        Player::open(
            shared(&decoder),
            Path::new("/bin/ffmpeg"),
            Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            true,
        )
        .is_err()
    );
    decoder.fail_probe.set(false);
    decoder.fail_start.set(true);
    assert!(
        Player::open(
            shared(&decoder),
            Path::new("/bin/ffmpeg"),
            Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            true,
        )
        .is_err()
    );
}

#[test]
fn drop_stops_stream() {
    let decoder = fake_decoder(info_of(640, 480, 10.0));
    let player = player(&decoder, true);
    assert!(decoder.streaming());
    drop(player);
    assert!(!decoder.streaming(), "the receiver is gone");
}
