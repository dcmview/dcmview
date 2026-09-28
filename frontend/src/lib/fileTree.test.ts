import { describe, expect, it } from "vitest";
import type { FileSummary } from "../api";
import {
	adjacentFileIndex,
	activeDirectoryPathKeys,
	activeStudyPathKeys,
	buildDirectoryTree,
	buildFileTree,
	directoryFileOrder,
	fileMatchesFilter,
	patientDetailWithCounts,
	studyFileOrder,
} from "./fileTree";

function file(overrides: Partial<FileSummary>): FileSummary {
	return {
		index: 0,
		path: "/study/image.dcm",
		label: "image",
		patient_id: "P-1",
		patient_name: "DOE^JANE",
		study_instance_uid: "1.2.3.study",
		study_date: "20260728",
		study_description: "Chest",
		series_instance_uid: "1.2.3.series",
		series_number: "1",
		series_description: "Axial",
		modality: "CT",
		instance_number: "1",
		sop_instance_uid: "1.2.3.image",
		sop_class_uid: "1.2.840.10008.5.1.4.1.1.2",
		object_kind: "classic_image",
		support_state: "renderable",
		support_reason: null,
		raw_windowing_compatible: true,
		raw_windowing_reason: null,
		presentation_layer: false,
		has_pixels: true,
		frame_count: 1,
		rows: 512,
		columns: 512,
		pixel_aspect_ratio: null,
		transfer_syntax_uid: "1.2.840.10008.1.2.1",
		default_window: null,
		...overrides,
	};
}

describe("file tree shaping", () => {
	it("produces identical hierarchy ordering for shuffled discovery results", () => {
		const files = [
			file({ index: 9, path: "/z/ten.dcm", patient_id: "P-2", patient_name: "Zulu^Patient", study_instance_uid: "study-old", study_date: "20200101", study_description: "Old", series_instance_uid: "series-10", series_number: "10", series_description: "Late", instance_number: "10" }),
			file({ index: 3, path: "/z/two.dcm", patient_id: "P-2", patient_name: "Zulu^Patient", study_instance_uid: "study-old", study_date: "20200101", study_description: "Old", series_instance_uid: "series-10", series_number: "10", series_description: "Late", instance_number: "2" }),
			file({ index: 4, path: "/z/new.dcm", patient_id: "P-2", patient_name: "Zulu^Patient", study_instance_uid: "study-new", study_date: "20250101", study_description: "New", series_instance_uid: "series-2", series_number: "2", series_description: "Early", instance_number: "1" }),
			file({ index: 1, path: "/a/known.dcm", patient_id: "P-1", patient_name: "Alpha^Patient", study_instance_uid: "alpha-study", study_date: "20240101", series_instance_uid: "alpha-series", series_number: "1", instance_number: "1" }),
			file({ index: 6, path: "/u/unknown.dcm", patient_id: "", patient_name: "", study_instance_uid: "", study_date: "", series_instance_uid: "", series_number: "", instance_number: "" }),
		];
		const orders = [
			files,
			[...files].reverse(),
			[files[2], files[4], files[0], files[3], files[1]],
		];
		const studyProjection = (input: FileSummary[]) => buildFileTree(input).map((patient) => ({
			key: patient.key,
			studies: patient.studies.map((study) => ({
				key: study.key,
				series: study.series.map((series) => ({
					key: series.key,
					files: series.files.map((item) => item.file.index),
				})),
			})),
		}));
		const directoryProjection = (input: FileSummary[]) => JSON.stringify(buildDirectoryTree(input));

		for (const order of orders.slice(1)) {
			expect(studyProjection(order)).toEqual(studyProjection(orders[0]));
			expect(directoryProjection(order)).toBe(directoryProjection(orders[0]));
		}
		expect(studyProjection(files)[0].key).toContain("P-1");
		expect(studyProjection(files)[1].studies.map((study) => study.key)).toEqual([
			"patient:P-2/study:study-new",
			"patient:P-2/study:study-old",
		]);
	});

	it("derives navigation order from the selected explorer organization", () => {
		const files = [
			file({ index: 1, path: "/z/first.dcm", patient_id: "A", patient_name: "Alpha", instance_number: "2" }),
			file({ index: 2, path: "/a/second.dcm", patient_id: "Z", patient_name: "Zulu", instance_number: "1" }),
			file({ index: 3, path: "/z/third.dcm", patient_id: "A", patient_name: "Alpha", instance_number: "1" }),
		];

		expect(studyFileOrder(buildFileTree(files))).toEqual([3, 1, 2]);
		expect(directoryFileOrder(buildDirectoryTree(files))).toEqual([2, 1, 3]);
	});

	it("selects adjacent files without wrapping or guessing missing selections", () => {
		const order = [8, 3, 5];
		expect(adjacentFileIndex(order, 3, -1)).toBe(8);
		expect(adjacentFileIndex(order, 3, 1)).toBe(5);
		expect(adjacentFileIndex(order, 8, -1)).toBeNull();
		expect(adjacentFileIndex(order, 5, 1)).toBeNull();
		expect(adjacentFileIndex(order, 99, 1)).toBeNull();
		expect(adjacentFileIndex(order, null, 1)).toBeNull();
	});

	it("groups by patient, study, and series while sorting numeric instances", () => {
		const tree = buildFileTree([
			file({ index: 42, instance_number: "10", path: "/study/ten.dcm" }),
			file({ index: 3, instance_number: "2", path: "/study/two.dcm" }),
			file({
				index: 900,
				series_instance_uid: "1.2.3.other",
				series_number: "2",
				series_description: "Coronal",
			}),
		]);

		expect(tree).toHaveLength(1);
		expect(tree[0].studies).toHaveLength(1);
		expect(tree[0].studies[0].series).toHaveLength(2);
		expect(tree[0].studies[0].series[0].files.map((item) => item.file.index))
			.toEqual([3, 42]);
		expect(patientDetailWithCounts(tree[0])).toContain("3 images");
	});

	it("keeps sibling studies and sibling series distinct within one patient", () => {
		const tree = buildFileTree([
			file({ index: 1, study_instance_uid: "study-a", study_description: "Baseline", series_instance_uid: "a-ax", series_description: "Axial" }),
			file({ index: 2, study_instance_uid: "study-a", study_description: "Baseline", series_instance_uid: "a-cor", series_description: "Coronal" }),
			file({ index: 3, study_instance_uid: "study-b", study_description: "Follow-up", series_instance_uid: "b-ax", series_description: "Axial" }),
		]);

		expect(tree).toHaveLength(1);
		expect(tree[0].studies.map((study) => study.label)).toEqual(["Baseline", "Follow-up"]);
		expect(tree[0].studies[0].series.map((series) => series.label)).toEqual(["Axial", "Coronal"]);
	});

	it("supports scoped terms and combines terms with AND semantics", () => {
		const image = file({});
		expect(fileMatchesFilter(image, "patient:jane modality:ct")).toBe(true);
		expect(fileMatchesFilter(image, "study:chest modality:mr")).toBe(false);
		expect(fileMatchesFilter(image, "series:axial")).toBe(true);
	});

	it("uses file identifiers for fallback keys without assuming density", () => {
		const tree = buildFileTree([
			file({
				index: 77,
				patient_id: "",
				patient_name: "",
				study_instance_uid: "",
				study_description: "",
				study_date: "",
				series_instance_uid: "",
				series_number: "",
				series_description: "",
			}),
		]);
		expect(tree[0].key).toBe("patient:file-77");
		expect(tree[0].studies[0].key).toContain("study:file-77");
		expect(tree[0].studies[0].series[0].key).toContain("series:file-77");
	});

	it("builds stable directory folders from file paths", () => {
		const tree = buildDirectoryTree([
			file({ index: 1, path: "/dataset/train/case-b.dcm" }),
			file({ index: 2, path: "/dataset/test/case-a.dcm" }),
		]);

		expect(tree[0]).toMatchObject({ kind: "folder", label: "dataset" });
		if (tree[0].kind !== "folder") throw new Error("expected dataset folder");
		expect(tree[0].children.map((node) => node.label)).toEqual(["test", "train"]);
	});

	it("trims machine-specific directory prefixes while retaining useful context", () => {
		const tree = buildDirectoryTree([
			file({ index: 1, path: "/srv/project/dataset/train/case.dcm" }),
			file({ index: 2, path: "/srv/project/dataset/validation/case.dcm" }),
		]);
		expect(tree[0]).toMatchObject({ kind: "folder", label: "dataset" });
	});

	it("identifies only the active patient, study, and series path", () => {
		const tree = buildFileTree([
			file({ index: 1, study_instance_uid: "study-a", series_instance_uid: "series-a" }),
			file({ index: 2, study_instance_uid: "study-b", series_instance_uid: "series-b" }),
		]);
		const keys = activeStudyPathKeys(tree, 2);

		expect(keys).toEqual(new Set([
			tree[0].key,
			tree[0].studies[1].key,
			tree[0].studies[1].series[0].key,
		]));
		expect(activeStudyPathKeys(tree, null)).toEqual(new Set());
	});

	it("identifies every ancestor folder of the active directory file", () => {
		const tree = buildDirectoryTree([
			file({ index: 1, path: "/dataset/train/nested/one.dcm" }),
			file({ index: 2, path: "/dataset/test/two.dcm" }),
		]);
		const keys = activeDirectoryPathKeys(tree, 1);

		expect(Array.from(keys)).toEqual(expect.arrayContaining([
			"directory:dataset",
			"directory:dataset/train",
			"directory:dataset/train/nested",
		]));
		expect(keys).not.toContain("directory:dataset/test");
	});
});
