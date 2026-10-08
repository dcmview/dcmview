from __future__ import annotations

import unittest
from pathlib import Path

from scripts import startup_timing as timing


def run(ms: float, files: int = 10) -> timing.Run:
	return timing.Run(
		first_response_ms=ms, first_file_ms=ms, scan_complete_ms=ms,
		catalog_ms=ms, catalog_bytes=100, file_count=files,
	)


class ThresholdTests(unittest.TestCase):
	def test_the_gate_compares_best_runs_with_a_percentage_and_a_floor(self) -> None:
		self.assertEqual((timing.THRESHOLD_PERCENT, timing.THRESHOLD_FLOOR_MS), (5.0, 3.0))
		profile = timing.PROFILES[0]
		cases = [
			# (baseline runs, candidate runs, passes)
			("within the percentage", [1000, 1200], [1052, 5000], True),
			("past the percentage and the floor", [1000, 1200], [1054, 1054], False),
			("a few milliseconds are under the floor", [4, 5], [7.1, 9], True),
			("past the floor", [4, 5], [7.3, 7.3], False),
			("a slow outlier does not fail a fast binary", [100, 100, 100], [100, 100, 900], True),
			("a fast outlier does not hide in the baseline", [100, 300, 300], [109, 109, 109], False),
		]
		for name, baseline, candidate, passes in cases:
			with self.subTest(name=name):
				result = timing.compare(profile, 10, [run(ms) for ms in baseline], [run(ms) for ms in candidate])
				gated = [metric for metric in result.metrics if metric.gated]
				self.assertEqual(len(gated), len(timing.GATED_METRICS))
				self.assertEqual(all(metric.passed for metric in gated), passes)
				# The catalog listing is reported and never fails a run.
				self.assertTrue(all(metric.passed for metric in result.metrics if not metric.gated))

	def test_binaries_that_list_different_files_are_not_compared(self) -> None:
		result = timing.compare(timing.PROFILES[0], 10, [run(100, files=4)], [run(900, files=10)])
		self.assertFalse(result.comparable)
		self.assertTrue(all(metric.passed for metric in result.metrics))


class SyntheticInputTests(unittest.TestCase):
	def test_generated_files_are_part_10_dicom_and_png_and_differ_per_instance(self) -> None:
		first = timing.dicom_file(1, 1, 1, 1)
		second = timing.dicom_file(1, 1, 1, 2)
		self.assertEqual(first[128:132], b"DICM")
		self.assertEqual(len(first) % 2, 0)
		self.assertNotEqual(first, second)
		self.assertEqual(len(first), len(second), "instances of a series are the same length")
		self.assertEqual(timing.png_file(0)[:8], b"\x89PNG\r\n\x1a\n")
		self.assertNotEqual(timing.png_file(0), timing.png_file(1))

	def test_the_baseline_is_a_released_tag_kept_under_target(self) -> None:
		self.assertEqual(timing.BASELINE_REF, "v0.4.0")
		self.assertEqual(timing.timing_root(), Path(timing.REPO_ROOT, "target", "timing"))


if __name__ == "__main__":
	unittest.main()
