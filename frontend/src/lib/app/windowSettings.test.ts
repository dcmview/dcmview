import { describe, expect, it } from "vitest";
import type { WindowPreset } from "../../api";
import { WindowSettings } from "./windowSettings.svelte";

const defaults = new Map<number, WindowPreset | null>([
	[1, { center: 100, width: 200 }],
	[2, { center: 1000, width: 400 }],
	[3, null],
]);

function settings(): WindowSettings {
	return new WindowSettings((fileIndex) => defaults.get(fileIndex));
}

function state(window: WindowSettings) {
	return { center: window.center, width: window.width, mode: window.mode, presetId: window.presetId };
}

describe("WindowSettings", () => {
	it("applies fixed-window and mode presets", () => {
		const window = settings();
		window.selectPreset("brain");
		expect(state(window)).toEqual({ center: 40, width: 80, mode: "default", presetId: "brain" });
		window.selectPreset("full_dynamic");
		expect(state(window)).toEqual({ center: null, width: null, mode: "full_dynamic", presetId: "full_dynamic" });
	});

	it("carries a manual window to the next file relative to its default", () => {
		const window = settings();
		window.selectPreset("full_dynamic");
		window.recordManual(1, 150, 100);
		expect(state(window)).toEqual({ center: 150, width: 100, mode: "default", presetId: "default" });

		window.followFile(2);
		// Center moved a quarter of the width up and the width halved, on file 2's scale.
		expect(state(window)).toMatchObject({ center: 1100, width: 200, mode: "default" });
	});

	it("does not carry a window without a usable default on either side", () => {
		const window = settings();
		window.recordManual(3, 50, 60);
		window.followFile(2);
		expect(state(window)).toMatchObject({ center: 50, width: 60 });

		window.recordManual(1, 150, 100);
		window.followFile(3);
		expect(state(window)).toMatchObject({ center: 150, width: 100 });
	});

	it("drops the manual adjustment on a preset or reset", () => {
		const window = settings();
		window.recordManual(1, 150, 100);
		window.selectPreset("default");
		window.followFile(2);
		expect(state(window)).toEqual({ center: null, width: null, mode: "default", presetId: "default" });

		window.recordManual(1, 150, 100);
		window.reset();
		window.followFile(2);
		expect(state(window)).toEqual({ center: null, width: null, mode: "default", presetId: "default" });
	});
});
