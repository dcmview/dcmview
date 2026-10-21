// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { fileSummary } from "../../testing/fixtures";
import type { InputDevice, InputProfileSetting } from "./inputProfile";
import { ToolHost, type ToolHostView } from "./ToolHost.svelte";
import type { ToolId } from "../annotation/tools/tool";
import { DEFAULT_ORIENTATION } from "../viewerTools";
import type { ViewTransform } from "./viewTransform";

/**
 * A host over a viewport that zooms, pans and steps through `frames`
 * images in `activeTool`; the override has no control in the viewer yet.
 */
function hostOverViewport({ frames = 1, activeTool = "pan" as ToolId } = {}) {
	let transform: ViewTransform = { scale: 1, tx: 0, ty: 0, fit: false };
	let position = 0;
	const view: ToolHostView = {
		file: fileSummary(5),
		frame: 0,
		imageRows: 64,
		imageColumns: 64,
		get transform() { return transform; },
		orientation: DEFAULT_ORIENTATION,
		setTransform: (next) => { transform = { ...next, fit: false }; },
		toImage: () => null,
		zoomAnchor: () => null,
		zoomTransform: () => null,
		navigation: {
			count: frames,
			get position() { return position; },
			go: (next) => { position = Math.max(0, Math.min(frames - 1, next)); },
		},
		window: { begin: () => null, preview: () => {}, commit: () => {} },
		rects: {
			editable: false, visible: [], annotations: null, selectedIndex: null, coversAllFrames: false,
			select: () => {}, beginLiveEdit: () => {}, showDraft: () => {}, commit: () => {},
		},
		activeTool,
		displayedFrameIsCurrent: true,
		viewportHeight: 600,
		zoomAt: (scale) => { transform = { ...transform, scale }; },
		scheduleProbe: () => {},
		gestureEnded: () => {},
	};
	return {
		host: new ToolHost(view),
		shown: () => ({ scale: Number(transform.scale.toFixed(3)), tx: transform.tx, ty: transform.ty }),
		position: () => position,
	};
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

/** A wheel event `at` ms into the test; happy-dom's WheelEvent lacks the modifier and time fields. */
function wheelEvent(deltaY: number, at: number, keys: { altKey?: boolean; ctrlKey?: boolean } = {}): WheelEvent {
	const event = new WheelEvent("wheel", { cancelable: true, deltaY });
	Object.defineProperties(event, {
		altKey: { value: keys.altKey ?? false },
		ctrlKey: { value: keys.ctrlKey ?? false },
		timeStamp: { value: 10_000 + at },
	});
	return event;
}

describe("ToolHost wheel gestures keep the meaning they began with", () => {
	// Constructed: fingers speeding up over 115 px, then the momentum the system adds.
	const fingers = [2, 5, 9, 14, 20, 22, 18, 25];
	const momentum = [20, 16, 13, 10, 8, 6, 5, 4, 3, 2, 2, 1, 1, 1];

	it.each<ToolId>(["pan", "scroll", "zoom"])("Alt let go during an Alt+two-finger swipe's momentum pans nothing (%s tool)", (tool) => {
		const held = hostOverViewport({ frames: 60, activeTool: tool });
		const released = hostOverViewport({ frames: 60, activeTool: tool });

		for (const [index, deltaY] of [...fingers, ...momentum].entries()) {
			held.host.wheel(wheelEvent(deltaY, index * 16, { altKey: true }));
			// Alt comes up once the fingers lift, while the momentum runs on.
			released.host.wheel(wheelEvent(deltaY, index * 16, { altKey: index < fingers.length }));
		}

		expect(held.position()).toBe(5);
		expect(released.position()).toBe(5);
		expect(released.shown()).toEqual({ scale: 1, tx: 0, ty: 0 });
	});

	it("a pinch whose Ctrl comes up before its last events neither pans nor steps", () => {
		const { host, shown, position } = hostOverViewport({ frames: 60, activeTool: "scroll" });

		host.wheel(wheelEvent(-10, 0, { ctrlKey: true }));
		host.wheel(wheelEvent(-6, 16));
		host.wheel(wheelEvent(-3, 32));

		expect(shown()).toEqual({ scale: Number(Math.exp(0.19).toFixed(3)), tx: 0, ty: 0 });
		expect(position()).toBe(0);
	});

	it("a swipe that began without Alt keeps panning when Alt goes down", () => {
		const { host, shown, position } = hostOverViewport({ frames: 60 });

		for (const [index, deltaY] of fingers.entries()) host.wheel(wheelEvent(deltaY, index * 16, { altKey: index >= 4 }));

		expect(position()).toBe(0);
		expect(shown()).toEqual({ scale: 1, tx: 0, ty: -115 });
	});

	it("the next gesture after a pause takes its own meaning", () => {
		const { host, shown, position } = hostOverViewport({ frames: 60 });

		for (const [index, deltaY] of fingers.entries()) host.wheel(wheelEvent(deltaY, index * 16, { altKey: true }));
		const stepped = position();
		host.wheel(wheelEvent(10, fingers.length * 16 + 400));

		expect(position()).toBe(stepped);
		expect(shown()).toEqual({ scale: 1, tx: 0, ty: -10 });
	});
});
