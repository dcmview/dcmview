import { describe, expect, it } from "vitest";
import { isEditableTarget, RepeatThrottle, shortcutFor, type ShortcutContext, type ShortcutKeyEvent } from "./keyboardShortcuts";

const idle: ShortcutContext = { drawerOpen: false, multiFrame: true, roiToolActive: false, annotationItems: false };

function press(key: string, overrides: Partial<ShortcutKeyEvent> = {}): ShortcutKeyEvent {
	return { key, target: null, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, ...overrides };
}

function element(tagName: string, isContentEditable = false): EventTarget {
	return { tagName, isContentEditable } as unknown as EventTarget;
}

describe("isEditableTarget", () => {
	it("treats form controls and contenteditable elements as editable", () => {
		expect(isEditableTarget(element("INPUT"))).toBe(true);
		expect(isEditableTarget(element("TEXTAREA"))).toBe(true);
		expect(isEditableTarget(element("SELECT"))).toBe(true);
		expect(isEditableTarget(element("DIV", true))).toBe(true);
		expect(isEditableTarget(element("BUTTON"))).toBe(false);
		expect(isEditableTarget(null)).toBe(false);
	});
});

describe("shortcutFor", () => {
	it("keeps range arrows native while allowing playback and tool shortcuts", () => {
		const target = { tagName: "INPUT", type: "range" } as unknown as EventTarget;
		expect(isEditableTarget(target)).toBe(false);
		expect(shortcutFor(press(" ", { target }), idle)).toEqual({ type: "toggle-cine" });
		expect(shortcutFor(press("w", { target }), idle)).toEqual({ type: "select-tool", tool: "window_level" });
		for (const key of ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"]) {
			expect(shortcutFor(press(key, { target }), idle)).toBeNull();
		}
	});

	it("selects tools by their toolbar letters, in either case", () => {
		expect(shortcutFor(press("w"), idle)).toEqual({ type: "select-tool", tool: "window_level" });
		expect(shortcutFor(press("P"), idle)).toEqual({ type: "select-tool", tool: "pan" });
		expect(shortcutFor(press("z"), idle)).toEqual({ type: "select-tool", tool: "zoom" });
		expect(shortcutFor(press("s"), idle)).toEqual({ type: "select-tool", tool: "scroll" });
		expect(shortcutFor(press("r"), idle)).toEqual({ type: "select-tool", tool: "annotate_rect" });
		expect(shortcutFor(press("x"), idle)).toEqual({ type: "select-tool", tool: "redact" });
	});

	it("moves between files with unmodified up and down arrows", () => {
		expect(shortcutFor(press("ArrowUp"), idle)).toEqual({ type: "select-adjacent-file", step: -1 });
		expect(shortcutFor(press("ArrowDown"), idle)).toEqual({ type: "select-adjacent-file", step: 1 });
		expect(shortcutFor(press("ArrowDown", { shiftKey: true }), idle)).toBeNull();
		expect(shortcutFor(press("ArrowUp", { metaKey: true }), idle)).toBeNull();
	});

	it("steps frames and toggles cine only for multi-frame tabs", () => {
		expect(shortcutFor(press("ArrowLeft"), idle)).toEqual({ type: "step-frame", step: -1 });
		expect(shortcutFor(press("["), idle)).toEqual({ type: "step-frame", step: -1 });
		expect(shortcutFor(press("ArrowRight"), idle)).toEqual({ type: "step-frame", step: 1 });
		expect(shortcutFor(press("]"), idle)).toEqual({ type: "step-frame", step: 1 });
		expect(shortcutFor(press(" "), idle)).toEqual({ type: "toggle-cine" });
		const single = { ...idle, multiFrame: false };
		expect(shortcutFor(press("ArrowRight"), single)).toBeNull();
		expect(shortcutFor(press(" "), single)).toBeNull();
	});

	it("deletes the selected ROI only while the ROI tool is active", () => {
		const roi = { ...idle, roiToolActive: true };
		expect(shortcutFor(press("Delete"), roi)).toEqual({ type: "delete-roi" });
		expect(shortcutFor(press("Backspace"), roi)).toEqual({ type: "delete-roi" });
		expect(shortcutFor(press("Delete"), idle)).toBeNull();
	});

	it("steps annotation items only while a presentation state with items is shown", () => {
		const annotated = { ...idle, annotationItems: true };
		expect(shortcutFor(press(","), annotated)).toEqual({ type: "step-annotation-item", step: -1 });
		expect(shortcutFor(press("."), annotated)).toEqual({ type: "step-annotation-item", step: 1 });
		expect(shortcutFor(press("."), idle)).toBeNull();
	});

	it("ignores every shortcut while typing, including in contenteditable", () => {
		const context = { drawerOpen: false, multiFrame: true, roiToolActive: true, annotationItems: true };
		for (const target of [element("INPUT"), element("TEXTAREA"), element("SELECT"), element("DIV", true)]) {
			for (const key of ["w", "ArrowDown", "ArrowRight", " ", "Backspace", "."]) {
				expect(shortcutFor(press(key, { target }), context), `${key} in ${String(target)}`).toBeNull();
			}
		}
	});

	it("closes an open drawer on Escape even from an input", () => {
		const drawer = { ...idle, drawerOpen: true };
		expect(shortcutFor(press("Escape", { target: element("INPUT") }), drawer)).toEqual({ type: "close-drawer" });
		expect(shortcutFor(press("Escape"), idle)).toBeNull();
	});
});

describe("RepeatThrottle", () => {
	it("lets first presses through and spaces out held-key repeats", () => {
		const throttle = new RepeatThrottle();
		const key = (timeStamp: number, repeat: boolean) => ({ key: "ArrowRight", repeat, timeStamp });

		expect(throttle.allow(key(0, false), 60)).toBe(true);
		expect(throttle.allow(key(33, true), 60)).toBe(false);
		expect(throttle.allow(key(66, true), 60)).toBe(true);
		expect(throttle.allow(key(99, true), 60)).toBe(false);
		// A fresh press is never throttled.
		expect(throttle.allow(key(100, false), 60)).toBe(true);
	});
});
