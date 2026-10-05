// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { fileSummary } from "../../testing/fixtures";
import type { InputDevice, InputProfileSetting } from "./inputProfile";
import { ToolHost, type ToolHostView } from "./ToolHost.svelte";
import type { ViewTransform } from "./viewTransform";

/** A host over a viewport that only zooms and pans; the override has no control in the viewer yet. */
function hostOverViewport() {
	let transform: ViewTransform = { scale: 1, tx: 0, ty: 0, fit: false };
	const view: ToolHostView = {
		file: fileSummary(5),
		frame: 0,
		imageRows: 64,
		imageColumns: 64,
		get transform() { return transform; },
		setTransform: (next) => { transform = { ...next, fit: false }; },
		toImage: () => null,
		zoomAnchor: () => null,
		zoomTransform: () => null,
		navigation: { count: 1, position: 0, go: () => {} },
		window: { begin: () => null, preview: () => {}, commit: () => {} },
		rects: {
			editable: false, visible: [], annotations: null, selectedIndex: null, coversAllFrames: false,
			select: () => {}, beginLiveEdit: () => {}, showDraft: () => {}, commit: () => {},
		},
		activeTool: "pan",
		displayedFrameIsCurrent: true,
		viewportHeight: 600,
		zoomAt: (scale) => { transform = { ...transform, scale }; },
		scheduleProbe: () => {},
		gestureEnded: () => {},
	};
	return { host: new ToolHost(view), shown: () => ({ scale: Number(transform.scale.toFixed(3)), tx: transform.tx, ty: transform.ty }) };
}

const zoomedOut = (deltaY: number) => ({ scale: Number(Math.exp(-deltaY * 0.0025).toFixed(3)), tx: 0, ty: 0 });

describe("ToolHost input profile override", () => {
	it.each<[string, InputProfileSetting, WheelEventInit, InputDevice, { scale: number; tx: number; ty: number }]>([
		["Auto zooms on a wheel notch", "auto", { deltaY: 100 }, "mouse", zoomedOut(100)],
		["Auto pans on a lone small step in a session that has shown nothing", "auto", { deltaY: 20 }, "mouse", { scale: 1, tx: 0, ty: -20 }],
		["Trackpad pans even on a wheel notch", "trackpad", { deltaY: 100 }, "trackpad", { scale: 1, tx: 0, ty: -100 }],
		["Mouse zooms even on a small diagonal step", "mouse", { deltaX: 3, deltaY: 20 }, "mouse", zoomedOut(20)],
	])("%s", (_name, setting, wheel, profile, expected) => {
		const { host, shown } = hostOverViewport();
		host.inputProfileSetting = setting;

		host.wheel(new WheelEvent("wheel", { cancelable: true, ...wheel }));

		expect(host.inputProfileSetting).toBe(setting);
		expect(host.inputProfile).toBe(profile);
		expect(shown()).toEqual(expected);
	});

	it("returns to what the wheel has shown when set back to Auto", () => {
		const { host } = hostOverViewport();
		host.inputProfileSetting = "mouse";
		// A diagonal step is a trackpad's, whatever the override says.
		host.wheel(new WheelEvent("wheel", { cancelable: true, deltaX: 3, deltaY: 20 }));
		expect(host.inputProfile).toBe("mouse");

		host.inputProfileSetting = "auto";

		expect(host.inputProfile).toBe("trackpad");
	});
});
