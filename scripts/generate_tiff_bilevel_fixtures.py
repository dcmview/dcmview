#!/usr/bin/env python3
"""Write the bilevel TIFF files under tests/fixtures/tiff-bilevel.

Every file is honest output of libtiff, over an image this script computes
with integer arithmetic alone (`card` below; the tests compute the same
image to know what each pixel stores), so the same tools write the same
bytes again:

- Pillow 12.1.1 over libtiff 4.7.1 writes the image once in each
  compression: none, PackBits, LZW, Deflate and CCITT Group 4, as one strip,
  black is zero;
- `tiffcp` of libtiff 4.7.2 rewrites the uncompressed file in strips of four
  rows (`-r 4`), with `FillOrder` 2 (`-f lsb2msb`) and in 16 x 16 tiles
  (`-t -w 16 -l 16`);
- `ppm2tiff` of libtiff 4.7.2 writes the image from a PBM file, which makes
  it white is zero;
- Pillow writes one more Group 4 file, of other stripes (`stripes`), for the
  tests that read a Group 4 strip as narrower than it was written.

The test files that are not honest (a truncated strip, a byte count past
the file, a declared width or predictor that differs) are made by the tests
from these, by editing a count or a tag. Run from the repository root:

    python scripts/generate_tiff_bilevel_fixtures.py

It needs Pillow (12.1.1 links libtiff 4.7.1), and `tiffcp` and `ppm2tiff`
on the PATH. It is not part of any check profile: the committed files are
the fixtures, and this is how they were made.
"""

from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path

from PIL import Image

OUTPUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "tiff-bilevel"

# (file stem, width, height)
SMALL = ("card", 37, 21)
LARGE = ("card", 4096, 3072)


def card(width: int, height: int) -> list[int]:
	"""A bilevel test card, one value (0 or 1) a pixel, row by row: diagonal
	stripes over the left half (one pixel wide in a small card, 64 in a card
	4,096 wide), a disc on the right half, and the last column set.
	`tests/integration/raster_bilevel.rs` computes the same."""
	unit = max(1, width // 64)
	centre_x, centre_y = width * 3 // 4, height // 2
	radius = min(width, height) // 4
	values = []
	for row in range(height):
		for column in range(width):
			if column == width - 1:
				value = 1
			elif column < width // 2:
				value = 1 if (column * 3 + row * 5) // unit % 7 < 3 else 0
			else:
				inside = (column - centre_x) ** 2 + (row - centre_y) ** 2 <= radius * radius
				value = 1 if inside else 0
			values.append(value)
	return values


def stripes(width: int, height: int) -> list[int]:
	"""A white first row, then stripes two pixels wide that move one pixel
	to the left from each row to the next: after its first striped row a
	Group 4 encoder writes each row as one vertical code a stripe edge."""
	return [
		1 if row > 0 and (column + row) % 4 < 2 else 0
		for row in range(height)
		for column in range(width)
	]


def image(width: int, height: int, values: list[int] | None = None) -> Image.Image:
	picture = Image.new("1", (width, height))
	picture.putdata([value * 255 for value in (values or card(width, height))])
	return picture


def tiffcp(source: Path, target: Path, *options: str) -> None:
	subprocess.run(["tiffcp", *options, str(source), str(target)], check=True)


def main() -> None:
	OUTPUT.mkdir(parents=True, exist_ok=True)
	compressions = {
		"none": "raw",
		"packbits": "packbits",
		"lzw": "tiff_lzw",
		"deflate": "tiff_adobe_deflate",
		"g4": "group4",
	}
	libtiff = {"none": "none", "packbits": "packbits", "lzw": "lzw", "deflate": "zip", "g4": "g4"}
	_, width, height = SMALL
	small = image(width, height)
	size = f"{width}x{height}"
	for name, pillow in compressions.items():
		small.save(OUTPUT / f"{name}-{size}.tif", compression=pillow)
	source = OUTPUT / f"none-{size}.tif"
	for name, option in libtiff.items():
		tiffcp(source, OUTPUT / f"{name}-strips-{size}.tif", "-c", option, "-r", "4")
		tiffcp(source, OUTPUT / f"{name}-fill-order-2-{size}.tif", "-c", option, "-f", "lsb2msb")
		tiffcp(source, OUTPUT / f"{name}-tiles-{size}.tif", "-c", option, "-t", "-w", "16", "-l", "16")
	with tempfile.TemporaryDirectory() as scratch:
		pbm = Path(scratch) / "card.pbm"
		small.save(pbm)
		subprocess.run(
			["ppm2tiff", "-c", "g4", "-r", "4", str(pbm), str(OUTPUT / f"g4-white-is-zero-{size}.tif")],
			check=True,
		)
	_, width, height = LARGE
	image(width, height).save(OUTPUT / f"g4-{width}x{height}.tif", compression="group4")
	image(4096, 4, stripes(4096, 4)).save(OUTPUT / "g4-stripes-4096x4.tif", compression="group4")


if __name__ == "__main__":
	main()
