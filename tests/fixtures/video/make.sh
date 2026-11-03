#!/bin/sh
# Rebuilds the HDR10 video fixtures of src/player/probe/mod.rs.
#
# Needs ffmpeg 9 with libx265. Run it from any directory: ./tests/fixtures/video/make.sh
#
# Each file has 3 frames of 64x64 pixels (the ffmpeg test pattern), 10 bit HEVC, PQ (smpte2084)
# with the BT.2020 primaries. Only the HDR10 metadata differs:
#   hdr10-container.mp4: the `mdcv` and `clli` boxes of the sample entry: MaxCLL 4000 nits and a
#     mastering display maximum of 1000 nits, so the test shows that MaxCLL wins over the display.
#     ffmpeg reads the input options -mastering_display and -content_light and the mp4 muxer
#     writes the boxes. Matroska is not needed.
#   hdr10-sei.mp4: no box in the container. The SEI of each frame holds MaxCLL 2000 nits and a
#     mastering display maximum of 4000 nits (libx265 master-display and max-cll). The probe
#     finds it only with the second ffprobe run on the first frame.
set -eu

cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

pattern='testsrc2=size=64x64:rate=5'
encode='-frames:v 3 -pix_fmt yuv420p10le -vf setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc -c:v libx265 -color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc'
primaries='G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)'

# shellcheck disable=SC2086
ffmpeg -v error -y \
  -mastering_display "${primaries}L(10000000,1)" -content_light "4000,400" \
  -f lavfi -i "$pattern" $encode -x265-params "log-level=error:info=0" \
  "$work/container.mp4"

# shellcheck disable=SC2086
ffmpeg -v error -y -f lavfi -i "$pattern" $encode \
  -x265-params "log-level=error:info=0:master-display=${primaries}L(40000000,1):max-cll=2000,200" \
  "$work/sei.mp4"

cp "$work/container.mp4" hdr10-container.mp4
cp "$work/sei.mp4" hdr10-sei.mp4
