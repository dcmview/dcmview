import { describe, expect, it } from "vitest";
import type { FrameValueMapping } from "../../api";
import { renderRawFrameToRgba, samplePresentation } from "../rawWindowing";
import { frameDisplayWindowOptions, mappedWindowScale, windowToRender } from "./valueMapping";
import { rawFrame } from "../../testing/fixtures";
import { resolveWindow, type WindowInput } from "./resolveWindow";

const mapping: FrameValueMapping = {
	file_index: 1, frame_index: 0, stored_value_type: "integer",
	modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: null },
	real_world: [], voi_lut: null,
};
const input: WindowInput = {
	raw: rawFrame(1, 4), mapping, requested: null, live: null, mode: "default",
	defaultWindow: null, server: null, unitRequest: false,
};
const requested = { window: { wc: 30, ww: 60 }, unit: null };
const mapped: FrameValueMapping = { ...mapping, real_world: [{
	source: "real_world_value_mapping", source_file_index: null, label: "T1",
	first_value_mapped: 0, last_value_mapped: 3, unit_label: "ms", units: null, quantity: null,
	transform: { kind: "lut", values: [0, 0.1, 0.2, 0.3] },
}] };

describe("resolveWindow", () => {
	it("lets a live drag override automatic presentation, then uses the selected window", () => {
		const live = { window: { wc: 20, ww: 40 }, unit: null };
		expect(resolveWindow({ ...input, mode: "full_dynamic", requested, live })).toEqual({ ...live, source: "live" });
		expect(resolveWindow({ ...input, requested })).toEqual({ ...requested, source: "explicit" });
	});

	it("distinguishes explicit, DICOM and full-range windows", () => {
		const raw = rawFrame(1, 4);
		new Uint8Array(raw.buffer).set([0, 30, 60, 90]);
		raw.metadata.defaultWc = 45; raw.metadata.defaultWw = 80;
		expect(resolveWindow({ ...input, raw })).toEqual({ window: { wc: 45, ww: 80 }, unit: null, source: "dicom" });
		expect(resolveWindow({ ...input, raw, requested, mode: "full_dynamic" })).toEqual({ window: { wc: 45, ww: 90 }, unit: null, source: "full_dynamic" });
	});

	it("keeps a sub-unit real-world LUT window in its unit", () => {
		const choice = { window: { wc: 0.15, ww: 0.2 }, unit: "ms" };
		expect(resolveWindow({ ...input, mapping: mapped, requested: choice })).toEqual({ ...choice, source: "explicit" });
	});

	it("converts the resolved Modality window once into a linear mapping's unit", () => {
		const linear: FrameValueMapping = { ...mapped, real_world: [{ ...mapped.real_world[0], transform: { kind: "linear", slope: 0.5, intercept: -10 } }] };
		const choice = { window: { wc: 40, ww: 100 }, unit: "ms" };
		expect(resolveWindow({ ...input, mapping: linear, requested: choice })).toEqual({ ...choice, source: "explicit" });
		expect(resolveWindow({ ...input, raw: null, mapping: linear, server: { window: { wc: 100, ww: 200 }, appliedWindow: "linear" } })).toEqual({ ...choice, source: "server" });
	});

	it.each([
		["real_world", null, { window: { wc: 0.15, ww: 0.2 }, unit: "ms", source: "real_world" }],
		["voi_lut", null, { window: null, unit: null, source: "voi_lut" }],
		["linear", { wc: 10, ww: 20 }, { window: { wc: 10, ww: 20 }, unit: null, source: "fallback" }],
		[null, null, { window: null, unit: null, source: "color" }],
	] as const)("trusts only the server's real_world confirmation (%s)", (appliedWindow, window, expected) => {
		expect(resolveWindow({ ...input, raw: null, mapping: mapped, requested: { window: { wc: 0.15, ww: 0.2 }, unit: "ms" }, unitRequest: true, server: { appliedWindow, window } })).toEqual(expected);
	});

	it("preserves continuous gray levels through a sub-unit linear unit conversion", () => {
		const raw = rawFrame(1, 4); new Uint8Array(raw.buffer).set([0, 1, 2, 3]);
		const linear: FrameValueMapping = { ...mapped, real_world: [{ ...mapped.real_world[0], transform: { kind: "linear", slope: 0.1, intercept: 0 } }] };
		const choice = { window: { wc: 0.15, ww: 0.2 }, unit: "ms" };
		const scale = mappedWindowScale(linear)!;
		const window = windowToRender({ center: choice.window.wc, width: choice.window.ww }, scale, true);
		const rgba = renderRawFrameToRgba(raw, window.center, window.width, { presentation: samplePresentation(raw, linear) });
		expect([0, 1, 2, 3].map(i => rgba[i * 4])).toEqual([0, 64, 191, 255]);
		const options = frameDisplayWindowOptions({ wc: 0.15, ww: 0.2, unit: "ms" }, linear);
		expect(options).toEqual({ wc: window.center, ww: window.width, windowMode: "default" });
		expect(resolveWindow({ ...input, raw: null, mapping: linear, requested: choice,
			server: { window: { wc: window.center, ww: window.width }, appliedWindow: "linear" } })).toMatchObject({ unit: "ms" });
		const resolved = resolveWindow({ ...input, raw, mapping: linear, requested: choice });
		expect(resolved.window?.wc).toBeCloseTo(0.15);
		expect(resolved.window?.ww).toBeCloseTo(0.2);
	});

	it("never assigns a scalar window to raw color samples", () => {
		const raw = rawFrame(); raw.metadata.samplesPerPixel = 3;
		expect(resolveWindow({ ...input, raw, requested, live: requested })).toEqual({ window: null, unit: null, source: "color" });
	});
});
