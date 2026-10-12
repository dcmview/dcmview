#!/usr/bin/env python3
"""Write the codestreams under tests/fixtures/reduced-decode.

The frames are honest output of two encoders, over images this script
computes with integer arithmetic alone, so the same tools write the same
bytes again:

- JPEG 2000: `opj_compress` of OpenJPEG 2.5.4, reversible (lossless), with a
  fixed comment;
- JPEG: Pillow 12.1.1 over libjpeg-turbo, and for the one frame in the
  lossless process `cjpeg -lossless 1` of libjpeg-turbo 3.2.0.

The tests wrap these codestreams in DICOM files of their own, so the files
here are bare codestreams (and one JP2 file). Run from the repository root:

    python scripts/generate_reduced_decode_fixtures.py

It needs `opj_compress` and `cjpeg` on the PATH and Pillow. It is not part of any check
profile: the committed files are the fixtures, and this is how they were
made.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

OUTPUT = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "reduced-decode"
COMMENT = "dcmview reduced-decode fixture"


def phantom(width: int, height: int, top: int, seed: int, mirror: bool = False) -> list[int]:
	"""A test card with values from 0 to `top`: eight vertical bands that
	step from dark to light, discs with hard edges, bars along the top
	eighth, and a patch of fine texture. `mirror` flips it left to right."""
	state = (0x2545F491 ^ (seed * 0x9E3779B1)) & 0xFFFFFFFF
	smaller = min(width, height)
	discs = [
		(width * 3 // 10, height * 4 // 10, smaller // 5, top * 9 // 10),
		(width * 7 // 10, height * 5 // 10, smaller // 8, top // 10),
		(width * 5 // 10, height * 8 // 10, smaller // 10, top * 6 // 10),
	]
	bar = max(8, width // 40)
	values = []
	for row in range(height):
		line = []
		for column in range(width):
			value = top // 5 + (column * 8 // width) * (top // 20) + (row * 4 // height) * (top // 40)
			for centre_x, centre_y, radius, level in discs:
				if (column - centre_x) ** 2 + (row - centre_y) ** 2 <= radius * radius:
					value = level
			if row < height // 8 and (column // bar) % 2 == 0:
				value = top * 4 // 5
			state ^= (state << 13) & 0xFFFFFFFF
			state ^= state >> 17
			state ^= (state << 5) & 0xFFFFFFFF
			if column >= width * 13 // 16 and row >= height * 13 // 16:
				value += (state % 7 - 3) * max(1, top // 255)
			line.append(max(0, min(top, value)))
		values.extend(reversed(line) if mirror else line)
	return values


def colour(width: int, height: int, top: int, seed: int) -> list[int]:
	"""One phantom as three channels that differ, pixel by pixel: R, G, B."""
	gray = phantom(width, height, top, seed)
	return [
		sample
		for value in gray
		for sample in (value, top // 8 + value * 3 // 4, top - value)
	]


def raw_bytes(samples: list[int], depth: int, offset: int = 0) -> bytes:
	"""Big-endian samples as `opj_compress` reads a `.raw` file."""
	if depth <= 8 and offset == 0:
		return bytes(samples)
	return b"".join(
		(sample - offset).to_bytes(2, "big", signed=offset != 0) for sample in samples
	)


def jpeg_2000(
	name: str,
	size: tuple[int, int],
	components: int,
	depth: int,
	options: list[str],
	*,
	signed: bool = False,
	seed: int = 1,
	mirror: bool = False,
) -> None:
	width, height = size
	top = (1 << (depth - 1 if signed else depth)) - 1
	samples = phantom(width, height, top, seed, mirror)
	if components == 3:
		# A raw file holds one component after another.
		pixels = colour(width, height, top, seed)
		samples = pixels[0::3] + pixels[1::3] + pixels[2::3]
	offset = (top + 1) // 2 if signed else 0
	with tempfile.TemporaryDirectory() as work:
		source = Path(work) / "source.raw"
		source.write_bytes(raw_bytes(samples, depth, offset))
		subprocess.run(
			[
				"opj_compress",
				"-i",
				str(source),
				"-o",
				str(OUTPUT / name),
				"-F",
				f"{width},{height},{components},{depth},{'s' if signed else 'u'}",
				"-C",
				COMMENT,
				"-threads",
				"1",
				*options,
			],
			check=True,
			stdout=subprocess.DEVNULL,
		)


def jpeg(
	name: str,
	size: tuple[int, int],
	components: int,
	*,
	seed: int = 1,
	mirror: bool = False,
	**options: object,
) -> None:
	width, height = size
	if components == 3:
		image = Image.frombytes("RGB", size, bytes(colour(width, height, 255, seed)))
	else:
		image = Image.frombytes("L", size, bytes(phantom(width, height, 255, seed, mirror)))
	image.save(OUTPUT / name, "JPEG", quality=80, **options)


def lossless_jpeg(name: str, size: tuple[int, int], seed: int) -> None:
	width, height = size
	with tempfile.TemporaryDirectory() as work:
		source = Path(work) / "source.pgm"
		source.write_bytes(
			f"P5\n{width} {height}\n255\n".encode() + bytes(phantom(width, height, 255, seed))
		)
		subprocess.run(
			["cjpeg", "-lossless", "1", "-outfile", str(OUTPUT / name), str(source)],
			check=True,
		)


def main() -> int:
	OUTPUT.mkdir(parents=True, exist_ok=True)
	small, large = (512, 384), (1040, 780)

	# JPEG 2000. `-n` is the number of resolutions: one more than the
	# decomposition levels.
	six = ["-n", "6"]
	jpeg_2000("gray8-512x384.j2k", small, 1, 8, six)
	jpeg_2000("gray8-512x384-mirrored.j2k", small, 1, 8, six, mirror=True)
	jpeg_2000("gray8-512x384.jp2", small, 1, 8, six)
	jpeg_2000(
		"gray12-tiles-517x389.j2k",
		(517, 389),
		1,
		12,
		["-n", "5", "-t", "150,131", "-c", "[64,64],[64,64],[32,32]", "-p", "RPCL"],
		seed=2,
	)
	jpeg_2000("gray16-signed-512x384.j2k", small, 1, 16, six, signed=True, seed=3)
	jpeg_2000("gray8-offset-517x389.j2k", (517, 389), 1, 8, [*six, "-d", "7,5"], seed=4)
	jpeg_2000("gray8-offset-x-517x389.j2k", (517, 389), 1, 8, [*six, "-d", "7,0"], seed=4)
	jpeg_2000("gray8-offset-y-517x389.j2k", (517, 389), 1, 8, [*six, "-d", "0,5"], seed=4)
	jpeg_2000("gray8-no-levels-512x384.j2k", small, 1, 8, ["-n", "1"], seed=5)
	jpeg_2000("rgb8-rct-512x384.j2k", small, 3, 8, [*six, "-mct", "1"], seed=6)
	jpeg_2000("rgb8-plain-512x384.j2k", small, 3, 8, [*six, "-mct", "0"], seed=7)
	jpeg_2000("rgb12-rct-512x384.j2k", small, 3, 12, [*six, "-mct", "1"], seed=8)
	jpeg_2000(
		"gray12-tile-parts-1040x780.j2k",
		large,
		1,
		12,
		[*six, "-t", "512,512", "-TP", "R"],
		seed=9,
	)
	jpeg_2000("gray8-one-level-1040x780.j2k", large, 1, 8, ["-n", "2"], seed=10)
	jpeg_2000("gray12-2080x1560.j2k", (2080, 1560), 1, 12, six, seed=11)

	# JPEG. Pillow writes baseline (SOF0) unless told progressive (SOF2).
	jpeg("gray8-baseline-1040x780.jpg", large, 1, seed=12)
	jpeg("gray8-baseline-1040x780-mirrored.jpg", large, 1, seed=12, mirror=True)
	jpeg("gray8-progressive-1041x779.jpg", (1041, 779), 1, seed=13, progressive=True)
	jpeg("gray8-restart-2080x1560.jpg", (2080, 1560), 1, seed=14, restart_marker_blocks=64)
	jpeg("rgb8-420-1041x779.jpg", (1041, 779), 3, seed=15, subsampling="4:2:0")
	jpeg(
		"rgb8-444-progressive-1040x780.jpg",
		large,
		3,
		seed=16,
		subsampling="4:4:4",
		progressive=True,
	)
	lossless_jpeg("gray8-lossless-512x384.jpg", small, 17)
	return 0


if __name__ == "__main__":
	sys.exit(main())
