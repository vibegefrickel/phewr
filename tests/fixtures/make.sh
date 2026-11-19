#!/bin/sh
# Rebuilds the four animated fixtures from the FFmpeg testsrc pattern. The four still fixtures
# are in tuilibr (../tuilibr/tests/fixtures/make.sh).
#
# Needs ImageMagick 7 (magick), libavif (avifenc), ffmpeg and libwebp (img2webp).
# Run it from any directory. The optional argument selects the output folder.
# ./tests/fixtures/make.sh [output-folder]
#
# Each picture is 16x12 pixels. The APNG plays once. The other animations repeat forever.
# The AVIS file also holds the still item that an AVIF still decoder shows.
#   anim.avif   AV1, yuv444p, 3 frames at 200 ms, plus a still item
#   anim.gif    GIF, bgra, 3 frames with delays of 100, 500 and 200 ms
#   anim.png    APNG, rgb24, 3 frames at 200 ms, one play
#   anim.webp   WebP, argb, 3 frames at 200 ms, lossless
set -eu

out=${1:-$(dirname "$0")}
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The bottom two rows of testsrc form a color ramp that changes in each frame.
ffmpeg -v error -y -f lavfi -i 'testsrc=size=16x12:rate=5' -frames:v 3 \
  "$work/frame-%d.png"

magick -delay 10 "$work/frame-1.png" -delay 50 "$work/frame-2.png" \
  -delay 20 "$work/frame-3.png" -loop 0 "$out/anim.gif"
ffmpeg -v error -y -framerate 5 -i "$work/frame-%d.png" -plays 1 -f apng "$out/anim.png"
img2webp -loop 0 -d 200 -lossless "$work/frame-1.png" \
  -d 200 -lossless "$work/frame-2.png" -d 200 -lossless "$work/frame-3.png" \
  -o "$out/anim.webp"
# Quality 30 and speed 6 reproduce the pixels of the committed AVIF fixture. Zero timestamps
# keep the animated AVIF byte for byte repeatable.
avifenc --yuv 444 --fps 5 -q 30 -s 6 --creation-time 0 --modification-time 0 \
  "$work/frame-1.png" "$work/frame-2.png" "$work/frame-3.png" "$out/anim.avif"
