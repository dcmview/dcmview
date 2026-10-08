from __future__ import annotations

import contextlib
import io
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


def failure(message: str, **flags: bool) -> timing.RunFailed:
	error = timing.RunFailed(message, **flags)
	error.status = 3
	error.stderr_tail = ["the last line it wrote"]
	return error


class ServingTests(unittest.TestCase):
	"""What a run says when a binary cannot serve a profile."""

	def measured(self, timed_run: object, base: Path | None = None) -> tuple[list[timing.ProfileResult], list[timing.Failure], str]:
		output = io.StringIO()
		with tempfile.TemporaryDirectory() as directory, mock.patch.dict(os.environ, {"DCMVIEW_TIMING_DIR": directory}), contextlib.redirect_stdout(output):
			with mock.patch.object(timing, "timed_run", timed_run) if timed_run else contextlib.nullcontext():
				results, failures = timing.measure(self.baseline, self.candidate, [timing.PROFILES[0]], 2, base)
		return results, failures, output.getvalue()

	baseline = Path("baseline")
	candidate = Path("candidate")

	@unittest.skipIf(os.name == "nt", "the stub is a shell script")
	def test_a_candidate_that_exits_at_once_fails_the_gate(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			stub = Path(directory, "dcmview-stub")
			stub.write_text("#!/bin/sh\necho 'stub: cannot start' >&2\nexit 3\n")
			stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
			self.baseline = self.candidate = stub
			results, failures, output = self.measured(None)
		self.assertEqual(results, [])
		self.assertEqual([(failure.profile, failure.binary) for failure in failures], [("small", "candidate")])
		# The evidence is printed: what the process said and how it ended.
		self.assertIn("exited with status 3", output)
		self.assertIn("stub: cannot start", output)
		for enforce in (False, True):
			with contextlib.redirect_stdout(io.StringIO()):
				self.assertEqual(timing.verdict(results, failures, enforce), timing.EXIT_GATE_FAILED)

	def test_only_a_baseline_that_lists_nothing_is_skipped(self) -> None:
		def dies(binary: str, error: timing.RunFailed, after: int = 0) -> object:
			calls = {"count": 0}

			def timed_run(run_binary: Path, folder: Path, arguments: tuple[str, ...] = ()) -> timing.Run:
				if run_binary.name == binary:
					calls["count"] += 1
					if calls["count"] > after:
						raise error
				return run(100)

			return timed_run

		def dies_once(binary: str) -> object:
			calls = {"count": 0}

			def timed_run(run_binary: Path, folder: Path, arguments: tuple[str, ...] = ()) -> timing.Run:
				if run_binary.name == binary:
					calls["count"] += 1
					if calls["count"] == 2:
						raise failure("stopped serving")
				return run(100)

			return timed_run

		cases = [
			# (what happens, timed_run, profiles compared, who failed, exit status)
			("both serve", dies("neither", failure("unused")), 1, [], 0),
			("the baseline lists none of the files", dies("baseline", failure("listed no file", listed_nothing=True)), 0, [], 0),
			("the candidate lists none of the files", dies("candidate", failure("listed no file", listed_nothing=True)), 0, ["candidate"], timing.EXIT_GATE_FAILED),
			("the baseline cannot serve", dies("baseline", failure("did not report server_started")), 0, ["baseline"], timing.EXIT_NOT_COMPARED),
			("the candidate dies in a timed run, once", dies_once("candidate"), 1, [], 0),
			("the candidate dies in every timed run", dies("candidate", failure("stopped serving"), after=1), 0, ["candidate"], timing.EXIT_GATE_FAILED),
			("the baseline dies in every timed run", dies("baseline", failure("stopped serving"), after=1), 0, ["baseline"], timing.EXIT_NOT_COMPARED),
		]
		for name, timed_run, compared, failed, status in cases:
			with self.subTest(name=name):
				results, failures, output = self.measured(timed_run)
				self.assertEqual(len(results), compared)
				self.assertEqual([failure.binary for failure in failures], failed)
				if failed:
					self.assertIn("exited with status 3", output)
					self.assertIn("the last line it wrote", output)
				with contextlib.redirect_stdout(io.StringIO()):
					self.assertEqual(timing.verdict(results, failures, enforce=True), status)

	def test_a_machine_without_a_local_port_is_an_error_and_not_a_skip(self) -> None:
		def timed_run(binary: Path, folder: Path, arguments: tuple[str, ...] = ()) -> timing.Run:
			raise failure("stopped serving: [Errno 49] Can't assign requested address", listed_nothing=True, no_port=True)

		with self.assertRaises(timing.NoLocalPort):
			self.measured(timed_run)
		self.assertNotEqual(timing.EXIT_NOT_COMPARED, timing.EXIT_GATE_FAILED)


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
