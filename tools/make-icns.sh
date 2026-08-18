#!/usr/bin/env bash
#
# Renders packaging/Muster.icns from packaging/icon.svg.
#
# Run this when the drawing changes, and commit what it produces. The .icns is
# checked in for the same reason licenses/herdr/LICENSE is: assembling a bundle
# has to stay offline and must not need a tool the project does not otherwise
# require. ImageMagick is that tool, and nobody should have to install it to run
# ./dev --bundle.
#
# The one bitmap that does not come from icon.svg is the 16x16, which comes from
# icon-16.svg instead - at that size a pane's outline and its interior land on
# the same pixel. Every entry at 32 and above renders the full drawing.
set -euo pipefail

cd "$(dirname "$0")/.."
svg=packaging/icon.svg
small=packaging/icon-16.svg
out=packaging/Muster.icns
set=$(mktemp -d)/Muster.iconset
trap 'rm -rf "$(dirname "$set")"' EXIT

if ! command -v magick >/dev/null; then
  echo "make-icns.sh needs ImageMagick, and there is none on this PATH." >&2
  echo "Impact: packaging/Muster.icns cannot be regenerated, so a change to" >&2
  echo "packaging/icon.svg will not reach the app bundle - which reads the" >&2
  echo ".icns and never the SVG. Install it with: brew install imagemagick" >&2
  exit 1
fi

mkdir -p "$set"
# name:pixels, the ten entries iconutil expects. @2x is twice the points, so
# 16x16@2x and 32x32 are both 32-pixel images of different drawings on purpose:
# one is read at 16 points on a retina display and can carry the detail.
render() { magick -background none "$1" -resize "$2x$2" "$set/icon_$3.png"; }
render "$small" 16   16x16
render "$svg"   32   16x16@2x
render "$svg"   32   32x32
render "$svg"   64   32x32@2x
render "$svg"   128  128x128
render "$svg"   256  128x128@2x
render "$svg"   256  256x256
render "$svg"   512  256x256@2x
render "$svg"   512  512x512
render "$svg"   1024 512x512@2x

iconutil -c icns "$set" -o "$out"
echo "wrote $out ($(wc -c < "$out" | tr -d ' ') bytes)"
