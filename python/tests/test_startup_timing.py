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

	def test_the_merge_base_is_reported_and_never_gated(self) -> None:
		profile = timing.PROFILES[0]
		# Far slower than the merge base and within the gate against the release.
		result = timing.compare(profile, 10, [run(100)], [run(104)], [run(50)])
		self.assertEqual(len(result.base_metrics), len(timing.GATED_METRICS) + len(timing.REPORTED_METRICS))
		self.assertTrue(all(not metric.gated and metric.passed for metric in result.base_metrics))
		self.assertEqual([round(metric.change_percent) for metric in result.base_metrics], [108] * len(result.base_metrics))
		self.assertTrue(all(metric.passed for metric in result.metrics))
		self.assertEqual(sorted(result.runs), ["base", "baseline", "candidate"])
		# The gate itself is unchanged by the third binary.
		slower = timing.compare(profile, 10, [run(100)], [run(900)], [run(900)])
		self.assertFalse(all(metric.passed for metric in slower.metrics))
		# A merge base that lists other files is left out, like a baseline that does.
		other = timing.compare(profile, 10, [run(100)], [run(104)], [run(50, files=3)])
		self.assertEqual(other.base_metrics, [])

	def test_resident_memory_is_the_median_of_the_runs_that_report_it(self) -> None:
		runs = [run(1), run(1), run(1), run(1)]
		for measured, kib in zip(runs, (2048, None, 4096, 3072)):
			measured.resident_kib = kib
		self.assertEqual(timing.resident_mib(runs), 3.0)
		self.assertIsNone(timing.resident_mib([run(1)]))


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

	def test_a_file_can_lack_its_uid_or_have_another_length(self) -> None:
		ordinary = timing.dicom_file(1, 1, 1, 1)
		sop_instance_uid = b"\x08\x00\x18\x00UI"
		self.assertIn(sop_instance_uid, ordinary)
		without = timing.dicom_file(1, 1, 1, 1, with_uid=False)
		self.assertNotIn(sop_instance_uid, without)
		self.assertEqual(without[128:132], b"DICM")
		longer = timing.dicom_file(1, 1, 1, 1, rows=33)
		self.assertIn(sop_instance_uid, longer)
		self.assertGreater(len(longer), len(ordinary), "the same instance at another length")
		self.assertEqual(len(longer) % 2, 0)

	def test_the_profiles_cover_the_key_cases(self) -> None:
		profiles = {profile.name: profile for profile in timing.PROFILES}
		for name in ("cohort", "duplicated", "collision", "no-uid", "masked"):
			self.assertIn(name, profiles)
		self.assertEqual(profiles["masked"].arguments, ("--mask",))
		self.assertEqual(profiles["masked"].folder_of, "tree", "the masked session reads the folder the plain one reads")
		self.assertEqual(profiles["tree"].arguments, ())

	def test_the_baseline_is_a_released_tag_kept_under_target(self) -> None:
		self.assertEqual(timing.BASELINE_REF, "v0.4.0")
		self.assertEqual(timing.timing_root(), Path(timing.REPO_ROOT, "target", "timing"))

	def test_a_kept_binary_is_named_for_its_compiler(self) -> None:
		version, _, commit = timing.toolchain().partition("-")
		self.assertRegex(version, r"^\d+\.\d+\.\d+")
		self.assertTrue(commit)


if __name__ == "__main__":
	unittest.main()
