import { describe, expect, it } from "vitest";
import type { TagNode } from "../api";
import {
	flattenTagRows,
	tagValueDisplay,
	tagValueToCopyText,
} from "./tagRows";

const tags: TagNode[] = [
	{
		tag: "(0008,0016)",
		keyword: "SOPClassUID",
		vr: "UI",
		value: { type: "string", value: "1.2.3" },
	},
	{
		tag: "(0008,1115)",
		keyword: "ReferencedSeriesSequence",
		vr: "SQ",
		value: {
			type: "sequence",
			items: [[{
				tag: "(0020,000E)",
				keyword: "SeriesInstanceUID",
				vr: "UI",
				value: { type: "string", value: "nested-series" },
			}]],
		},
	},
];

/** A raster file's metadata: flat leaves, then a group holding a group. */
const metadata: TagNode[] = [
	{ tag: "PNG:IHDR", keyword: "Width", vr: "", value: { type: "number", value: 640 } },
	{
		tag: "EXIF",
		keyword: "",
		vr: "",
		value: {
			type: "sequence",
			items: [[{
				tag: "IFD0",
				keyword: "",
				vr: "",
				value: {
					type: "sequence",
					items: [[
						{ tag: "0x010F", keyword: "Make", vr: "ASCII", value: { type: "string", value: "Aperture Works" } },
						{ tag: "0x0112", keyword: "Orientation", vr: "SHORT", value: { type: "number", value: 6 } },
					]],
				},
			}]],
		},
	},
];

describe("tag row shaping", () => {
	it("shows the groups of a raster's metadata open, and closes the ones toggled", () => {
		const tags = (toggled: string[], filter = "") =>
			flattenTagRows(metadata, "f7", new Set(toggled), filter).map((row) => row.node.tag);
		expect(tags([])).toEqual(["PNG:IHDR", "EXIF", "IFD0", "0x010F", "0x0112"]);
		expect(tags(["f7-1:item0-0"])).toEqual(["PNG:IHDR", "EXIF", "IFD0"]);
		expect(tags(["f7-1"])).toEqual(["PNG:IHDR", "EXIF"]);
		// A filter shows the entries that match, under the groups that hold them.
		expect(tags([], "aperture")).toEqual(["EXIF", "IFD0", "0x010F"]);

		const rows = flattenTagRows(metadata, "f7", new Set(), "");
		expect(rows.map((row) => row.depth)).toEqual([0, 0, 1, 2, 2]);
		expect(tagValueDisplay(rows[1], false)).toBe("[1 entry]");
		expect(tagValueDisplay(rows[2], false)).toBe("[2 entries]");
	});


	it("retains a sequence parent when a descendant matches the filter", () => {
		const rows = flattenTagRows(tags, "f42", new Set(), "nested-series");
		expect(rows.map((row) => row.node.keyword)).toEqual(["ReferencedSeriesSequence"]);
	});

	it("uses stable nested keys and depth for expanded sequences", () => {
		const rows = flattenTagRows(tags, "f42", new Set(["f42-1"]), "");
		expect(rows.map(({ key, depth }) => ({ key, depth }))).toEqual([
			{ key: "f42-0", depth: 0 },
			{ key: "f42-1", depth: 0 },
			{ key: "f42-1:item0-0", depth: 1 },
		]);
	});

	it("formats long, binary, and truncated values consistently", () => {
		const longRow = {
			key: "long",
			depth: 0,
			node: {
				tag: "(0010,0010)",
				keyword: "PatientName",
				vr: "PN",
				value: { type: "string" as const, value: "x".repeat(90) },
			},
		};
		expect(tagValueDisplay(longRow, false)).toBe(`${"x".repeat(80)}…`);
		expect(tagValueDisplay(longRow, true)).toBe("x".repeat(90));
		expect(tagValueToCopyText({
			type: "numbers",
			value: [1, 2],
			total: 5,
			truncated: true,
		})).toBe("1, 2 (first 2 of 5)");
	});
});
