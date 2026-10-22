#!/usr/bin/env python3
"""Write the JPEG-compressed TIFF files under tests/fixtures/tiff-jpeg.

Every file is honest output of libtiff, over an image this script computes
with integer arithmetic alone (`card` below), so the same tools write the
same bytes again:

- Pillow 12.1.1 over libtiff 4.7.1 writes the card as gray and as RGB, in
  the strips Pillow chooses;
- `tiffcp` of libtiff 4.7.2 rewrites the uncompressed card in strips of 16
  rows (`-r 16`, so the last strip has fewer rows) and in 128 x 128 tiles
  (`-t -w 128 -l 128`, so the tiles on the right and bottom edges reach
  past the image): gray, RGB (`-c jpeg:r`) and YCbCr with 2 x 2 chroma
  subsampling (`-c jpeg`, libtiff's default for colour). Every one of these
  files shares its quantization and Huffman tables among its strips or
  tiles in `JPEGTables`;
- libtiff writes no progressive JPEG, so for the progressive files
  `jpegtran -progressive` of libjpeg-turbo 3.2.0 rewrites the one strip or
  tile of a libtiff file, joined to its tables, as a progressive stream
  (the same coefficients, in jpegtran's default scans), and the file is
  that libtiff file with the stream in place of the strip or tile, what
  follows it moved and the offsets to it adjusted;
- in the same way, one RGB page's strip is replaced by libjpeg-turbo's
  `cjpeg -sample 1x1` coding of the card, which is YCbCr: a page that says
  RGB over a YCbCr-coded stream;
- `tiffcp` joins the uncompressed gray card and its JPEG strips into one
  file of two pages.

Beside the small files it writes what libtiff itself decodes each to
(`tiffcp -c zip:p9 -s`, which converts YCbCr to RGB as libtiff's JPEG
decoder does), losslessly compressed in one strip, once for each distinct
decode (files whose 8 x 8 blocks are the same decode to the same pixels),
and `libtiff-decodes.txt`, a line for each file naming its decode. So the
tests compare the viewer's decode with libtiff's without libtiff.

The larger files are for the cost tests, which compare nothing but sizes:
a 2,048 x 2,048 card without the checkerboard, at quality 50, as one
progressive YCbCr strip and as one progressive RGB strip, the same card
4,096 pixels square as one progressive gray tile and 63,550 x 88 as one
progressive RGB strip (both just within 16 MiB decoded), and a card 65,500 pixels wide (the widest libjpeg writes) in two YCbCr strips of 16
rows.

The test files that are not honest (a frame header larger or smaller than
its tile, other components or precision, larger tables, a larger tile) are
made by the tests from these, by editing a tag, a count or a frame header
byte. Run from the repository root:

    python scripts/generate_tiff_jpeg_fixtures.py

It needs Pillow (12.1.1 links libtiff 4.7.1), and `tiffcp` (libtiff 4.7.2)
and `jpegtran` and `cjpeg` (libjpeg-turbo 3.2.0) on the PATH. It is not part of any
check profile: the committed files are the fixtures, and this is how they
were made.
"""

from __future__ import annotations

import struct
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

OUTPUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "tiff-jpeg"

SMALL = (150, 133)


def card(width: int, height: int, checks: bool = True) -> list[tuple[int, int, int]]:
	"""An RGB test card, row by row: red rising to the right, green rising
	downwards, and blue a disc of 255 on a checkerboard of 8-pixel squares
	of 64 and 128 (on 96 without `checks`)."""
	centre_x, centre_y = width // 2, height // 2
	radius = min(width, height) // 3
	pixels = []
	for row in range(height):
		for column in range(width):
			red = column * 255 // max(1, width - 1)
			green = row * 255 // max(1, height - 1)
			if (column - centre_x) ** 2 + (row - centre_y) ** 2 <= radius * radius:
				blue = 255
			elif checks:
				blue = 64 + 64 * ((column // 8 + row // 8) % 2)
			else:
				blue = 96
			pixels.append((red, green, blue))
	return pixels


def image(width: int, height: int, checks: bool = True) -> Image.Image:
	picture = Image.new("RGB", (width, height))
	picture.putdata(card(width, height, checks))
	return picture


def tiffcp(source: Path, target: Path, *options: str) -> None:
	subprocess.run(["tiffcp", *options, str(source), str(target)], check=True)


def ifd_entries(data: bytes) -> tuple[str, int, dict[int, tuple[int, int, int, int]]]:
	"""The first page's entries: tag -> (position, type, count, value)."""
	order = "<" if data[:2] == b"II" else ">"
	(ifd,) = struct.unpack_from(order + "I", data, 4)
	(count,) = struct.unpack_from(order + "H", data, ifd)
	entries = {}
	for index in range(count):
		at = ifd + 2 + 12 * index
		tag, kind, values, value = struct.unpack_from(order + "HHII", data, at)
		entries[tag] = (at, kind, values, value)
	return order, ifd, entries


TYPE_SIZES = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}


def with_chunk(data: bytes, offset_tag: int, count_tag: int, stream: bytes) -> bytes:
	"""`data`, a classic TIFF of one strip or tile, with that strip's or
	tile's bytes replaced by `stream` in place: what follows it moves, and
	the header's, the page's and its tags' offsets past it move with it."""
	order, ifd, entries = ifd_entries(data)
	_, _, chunks, offset = entries[offset_tag]
	_, _, _, count = entries[count_tag]
	assert chunks == 1, "one strip or tile"
	end = offset + count
	shift = len(stream) - count
	moved = lambda position: position + shift if position >= end else position
	assert ifd >= end, "the page follows its strip or tile"
	out = bytearray(data[:offset] + stream + data[end:])
	struct.pack_into(order + "I", out, 4, moved(ifd))
	for tag, (at, kind, values, value) in entries.items():
		at = moved(at)
		if values * TYPE_SIZES[kind] > 4:
			struct.pack_into(order + "I", out, at + 8, moved(value))
		elif tag == count_tag:
			struct.pack_into(order + "I", out, at + 8, len(stream))
	return bytes(out)


def joined_chunk(data: bytes, offset_tag: int, count_tag: int) -> bytes:
	"""The one strip or tile of `data` joined to its tables."""
	_, _, entries = ifd_entries(data)
	_, _, tables_count, tables_offset = entries[347]
	offset, count = entries[offset_tag][3], entries[count_tag][3]
	tables = data[tables_offset : tables_offset + tables_count]
	return tables[:-2] + data[offset + 2 : offset + count]


def progressive(source: Path, target: Path, tiled: bool = False) -> None:
	"""`source`, a libtiff file of one JPEG strip or tile, with that strip
	or tile rewritten by `jpegtran -progressive`."""
	tags = (324, 325) if tiled else (273, 279)
	data = source.read_bytes()
	stream = subprocess.run(
		["jpegtran", "-copy", "none", "-progressive"],
		input=joined_chunk(data, *tags),
		capture_output=True,
		check=True,
	).stdout
	target.write_bytes(with_chunk(data, *tags, stream))


def references(names: list[str]) -> None:
	"""What libtiff decodes each of `names` to, losslessly compressed, kept
	once for each distinct decode, and `libtiff-decodes.txt`, which names
	the decode of each file."""
	kept: dict[bytes, str] = {}
	lines = []
	for name in names:
		target = OUTPUT / f"{name}.libtiff.tif"
		tiffcp(OUTPUT / f"{name}.tif", target, "-c", "zip:p9", "-s", "-r", "1024")
		decoded = target.read_bytes()
		if decoded in kept:
			target.unlink()
		else:
			kept[decoded] = target.name
		lines.append(f"{name}.tif {kept[decoded]}\n")
	(OUTPUT / "libtiff-decodes.txt").write_text("".join(lines))


def main() -> None:
	OUTPUT.mkdir(parents=True, exist_ok=True)
	width, height = SMALL
	size = f"{width}x{height}"
	names = []
	with tempfile.TemporaryDirectory() as scratch:
		scratch = Path(scratch)
		colour = image(width, height)
		gray = colour.convert("L")
		gray.save(OUTPUT / f"gray-{size}.tif", compression="jpeg")
		colour.save(OUTPUT / f"rgb-{size}.tif", compression="jpeg")
		names += [f"gray-{size}", f"rgb-{size}"]
		plain = {"gray": scratch / "gray.tif", "rgb": scratch / "rgb.tif"}
		gray.save(plain["gray"])
		colour.save(plain["rgb"])
		layouts = {"strips": ["-r", "16"], "tiles": ["-t", "-w", "128", "-l", "128"]}
		kinds = {"gray": ("gray", "jpeg"), "rgb": ("rgb", "jpeg:r"), "ycbcr": ("rgb", "jpeg")}
		for layout, options in layouts.items():
			for kind, (source, compression) in kinds.items():
				name = f"{kind}-{layout}-{size}"
				tiffcp(plain[source], OUTPUT / f"{name}.tif", "-c", compression, *options)
				names.append(name)
		for kind, (source, compression) in kinds.items():
			single = scratch / f"{kind}-single.tif"
			tiffcp(plain[source], single, "-c", compression, "-r", "144")
			name = f"{kind}-progressive-{size}"
			progressive(single, OUTPUT / f"{name}.tif")
			names.append(name)
		# An RGB page whose strip is YCbCr-coded, as cjpeg codes it: the
		# page says RGB, so the stream is decoded unconverted.
		single = scratch / "rgb-single.tif"
		colour.save(scratch / "card.ppm")
		stream = subprocess.run(
			["cjpeg", "-sample", "1x1", "-quality", "75", str(scratch / "card.ppm")],
			capture_output=True,
			check=True,
		).stdout
		name = f"rgb-ycbcr-coded-{size}"
		(OUTPUT / f"{name}.tif").write_bytes(with_chunk(single.read_bytes(), 273, 279, stream))
		names.append(name)
		references(names)
		# An uncompressed page, then a JPEG-compressed one of its layout.
		two = OUTPUT / f"gray-two-pages-{size}.tif"
		subprocess.run(
			["tiffcp", str(plain["gray"]), str(OUTPUT / f"gray-strips-{size}.tif"), str(two)],
			check=True,
		)

		large = image(2048, 2048, checks=False)
		large.save(scratch / "large.tif")
		for kind, compression in (("ycbcr", "jpeg:50"), ("rgb", "jpeg:r:50")):
			single = scratch / f"large-{kind}.tif"
			tiffcp(scratch / "large.tif", single, "-c", compression, "-r", "2048")
			progressive(single, OUTPUT / f"{kind}-progressive-2048x2048.tif")
		image(4096, 4096, checks=False).convert("L").save(scratch / "tile.tif")
		single = scratch / "tile-jpeg.tif"
		tiffcp(scratch / "tile.tif", single, "-c", "jpeg:50", "-t", "-w", "4096", "-l", "4096")
		progressive(single, OUTPUT / "gray-progressive-tile-4096x4096.tif", tiled=True)
		# As wide as decodes to just under 16 MiB at 88 rows.
		image(63550, 88, checks=False).save(scratch / "wide-rgb.tif")
		single = scratch / "wide-rgb-jpeg.tif"
		tiffcp(scratch / "wide-rgb.tif", single, "-c", "jpeg:r:50", "-r", "88")
		progressive(single, OUTPUT / "rgb-progressive-63550x88.tif")
		image(65500, 32, checks=False).save(scratch / "wide.tif")
		tiffcp(scratch / "wide.tif", OUTPUT / "ycbcr-strips-65500x32.tif", "-c", "jpeg:50", "-r", "16")


if __name__ == "__main__":
	main()
