import { describe, expect, it, vi } from "vitest";
import type { EmbedRoiAnnotations } from "../../api";
import type { NativeAnnotationsResponse } from "../../generated/api-types";
// What the real binary served for golden-masking-patient-a-us-1.dcm, loaded
// with --annotations and the sample document
// (tests/fixtures/annotation-documents/sample.json): the native read and the
// EMBED read of the same file, as recorded.
import recorded from "../../testing/recorded/sample-document-native-read.json";
import { visibleRois } from "../viewport/roiEditing";
import { NativeAnnotations } from "../viewport/nativeAnnotations.svelte";
import { classLegend, labelGroups, layerEntries, nativeView } from "./display";
import { showsMoreThanEmbed } from "./nativeAnnotations";

const native = recorded.native as unknown as NativeAnnotationsResponse;
const embed = recorded.embed as unknown as EmbedRoiAnnotations;

const CLASSES = [
	{ id: "mass", name: "Mass", color: "#E4572E" },
	{ id: "calcification", name: "Calcification", color: "#2E86AB" },
	{ id: "distortion", name: "Architectural distortion", color: "#F2A541" },
	{ id: "roi", name: "ROI", color: "#8E44AD" },
];
const SECOND_READER = "#17BEBB";

describe("the native read the server serves for the sample document", () => {
	it("is shown as native, by the store, although its schema has a roi class", async () => {
		expect(showsMoreThanEmbed(native)).toBe(true);
		const store = new NativeAnnotations({ load: vi.fn(async () => native) });

		store.ensureLoaded(0);
		for (let index = 0; index < 6; index += 1) await Promise.resolve();

		expect(store.status(0)).toBe("ready");
		expect(store.response(0)).toBe(native);
	});

	it("lists every class with its name and color, and both layers", () => {
		expect(classLegend(native).map(({ id, name, color }) => ({ id, name, color }))).toEqual(CLASSES);
		expect(layerEntries(native).map(({ name, color }) => ({ name, color })))
			.toEqual([{ name: "Annotations", color: null }, { name: "Second reader", color: SECOND_READER }]);
	});

	it("draws each record in its layer's color, else its class's, named by its class", () => {
		const view = nativeView(native, { frame: 0 }, visibleRois(embed, 0));
		const records = new Map(native.annotations.map((record) => [record.id, record]));

		expect(view.shapes).toHaveLength(native.annotations.length);
		for (const shape of view.shapes) {
			const record = records.get(shape.id)!;
			const classDef = CLASSES.find((entry) => entry.id === record.class)!;
			expect(shape.color).toBe(record.layer === "reader-2" ? SECOND_READER : classDef.color);
			expect(shape.text).toBe(classDef.name);
		}
		expect(new Set(view.shapes.map((shape) => shape.text))).toEqual(new Set(CLASSES.map((entry) => entry.name)));
	});

	it("stands for every EMBED rectangle of the file, so none is drawn twice", () => {
		const view = nativeView(native, { frame: 0 }, visibleRois(embed, 0));

		expect([...view.embedDrawn].sort()).toEqual(embed.roi_coords.map((_, index) => index));
	});

	it("lists the file and study labels", () => {
		expect(labelGroups(native, 0).map((group) => [group.title, group.labels.map((label) => label.field)]))
			.toEqual([["Study", ["Breast density"]], ["File", ["Image quality", "Note"]]]);
	});
});
