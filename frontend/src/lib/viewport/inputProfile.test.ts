import { describe, expect, it } from "vitest";
import { InputProfile, type InputDevice, type WheelSample } from "./inputProfile";

type Step = Omit<WheelSample, "timeStamp">;
/** One gesture's wheel events and the gap between them. */
type Trace = { steps: Step[]; everyMs: number };

const pixels = (deltaY: number, deltaX = 0): Step => ({ deltaX, deltaY, deltaMode: 0 });
/** Chrome and Safari also report the legacy delta: three per trackpad pixel, 120 per wheel notch. */
const withLegacy = (step: Step, wheelDeltaY: number): Step => ({ ...step, wheelDeltaY });

// Every trace below is CONSTRUCTED from the behaviour section 3.5 of
// docs/design/annotation-tools-ux.md describes. None was recorded from a
// device: no trackpad or mouse hardware was available when they were written.
const FINGERS = [1, 3, 7, 12, 18, 22, 19, 15];
const MOMENTUM = [13, 11, 9, 8, 7, 6, 5, 4, 3, 3, 2, 2, 1, 1, 1];
const traces = {
	/** macOS trackpad, two-finger swipe then the momentum the system adds, in Chrome. */
	trackpadSwipe: { everyMs: 16, steps: [...FINGERS, ...MOMENTUM].map((dy) => withLegacy(pixels(dy), -3 * dy)) },
	/** The same swipe from a browser without the legacy delta. */
	trackpadSwipePlain: { everyMs: 16, steps: [...FINGERS, ...MOMENTUM].map((dy) => pixels(dy)) },
	/** A fast flick whose steps pass through a notch-sized 100. */
	trackpadFlick: { everyMs: 8, steps: [4, 31, 100, 120, 96, 80, 66, 54, 44].map((dy) => pixels(dy)) },
	/** Trackpad pinch: small fractional steps (the browser adds ctrlKey, which the classifier does not read). */
	pinch: { everyMs: 16, steps: [-0.5, -1.25, -2.75, -3.2, -2.1, -0.9].map((dy) => pixels(dy)) },
	/** Apple Magic Mouse: a touch surface, so trackpad-like steps with sideways drift and momentum. */
	magicMouse: { everyMs: 16, steps: [[2, 0], [6, 1], [11, 1], [15, -1], [12, 0], [9, 0], [7, 0], [5, 0], [3, 0], [1, 0]].map(([dy, dx]) => pixels(dy, dx)) },
	/** Notched wheel, 100 px a notch (Chrome on Windows), one notch. */
	notch100: { everyMs: 60, steps: [withLegacy(pixels(100), -120)] },
	/** Notched wheel, 120 px a notch, three notches spun quickly. */
	notch120: { everyMs: 40, steps: [pixels(-120), pixels(-120), pixels(-240)] },
	/** Line-mode wheel, as Firefox on Windows sends: three lines a notch. */
	lineMode: { everyMs: 60, steps: [{ deltaX: 0, deltaY: 3, deltaMode: 1 }, { deltaX: 0, deltaY: 3, deltaMode: 1 }] },
	/** Free-spinning high-resolution wheel: small pixel steps, each a multiple of one quantum. */
	freeSpin: { everyMs: 10, steps: [12.5, 12.5, 25, 37.5, 25, 25, 12.5, 12.5].map((dy) => pixels(dy)) },
	/** Notched mouse on macOS in Chrome: a few accelerated pixels per notch, but a whole legacy notch. */
	macMouseNotch: { everyMs: 60, steps: [withLegacy(pixels(4.000244140625), -120)] },
	/** One small pixel step with nothing around it: could be either device. */
	loneSmallStep: { everyMs: 16, steps: [pixels(20)] },
	/** Lone pixel steps either side of the 50 px that splits them while the session has shown nothing. */
	lone49: { everyMs: 16, steps: [pixels(49)] },
	lone50: { everyMs: 16, steps: [pixels(-50)] },
	lone99: { everyMs: 16, steps: [pixels(99)] },
	/** Shift+wheel on Windows and Linux: a whole notch sideways. */
	shiftWheel: { everyMs: 60, steps: [pixels(0, 100)] },
} satisfies Record<string, Trace>;

type TraceName = keyof typeof traces;
/** A gesture, the device every one of its events must act as, and the detected profile after it. */
type Expectation = [trace: TraceName, actsAs: InputDevice, detectedAfter: InputDevice | null];

const cases: { name: string; gestures: Expectation[] }[] = [
	{ name: "a trackpad swipe with momentum is a trackpad from its first event", gestures: [["trackpadSwipe", "trackpad", "trackpad"]] },
	{ name: "a swipe without the legacy delta is a trackpad", gestures: [["trackpadSwipePlain", "trackpad", "trackpad"]] },
	{ name: "a pinch is a trackpad", gestures: [["pinch", "trackpad", "trackpad"]] },
	{ name: "a Magic Mouse is a trackpad", gestures: [["magicMouse", "trackpad", "trackpad"]] },
	{ name: "a 100 px notch is a mouse", gestures: [["notch100", "mouse", "mouse"]] },
	{ name: "120 px notches are a mouse", gestures: [["notch120", "mouse", "mouse"]] },
	{ name: "a line-mode wheel is a mouse", gestures: [["lineMode", "mouse", "mouse"]] },
	{ name: "a macOS mouse notch of a few pixels is a mouse", gestures: [["macMouseNotch", "mouse", "mouse"]] },
	{
		name: "a free-spinning wheel is a mouse once its first steps show their quantum",
		gestures: [["freeSpin", "trackpad", "mouse"], ["freeSpin", "mouse", "mouse"]],
	},
	{
		name: "a lone small step decides nothing and follows the profile",
		gestures: [["loneSmallStep", "trackpad", null], ["notch100", "mouse", "mouse"], ["loneSmallStep", "mouse", "mouse"], ["loneSmallStep", "mouse", "mouse"]],
	},
	{ name: "in a fresh session a lone 49 px step acts as a trackpad", gestures: [["lone49", "trackpad", null]] },
	{ name: "in a fresh session a lone 50 px step acts as a mouse", gestures: [["lone50", "mouse", null]] },
	{ name: "in a fresh session a lone 99 px step acts as a mouse", gestures: [["lone99", "mouse", null]] },
	{
		name: "a sideways notch decides nothing",
		gestures: [["notch100", "mouse", "mouse"], ["shiftWheel", "mouse", "mouse"], ["shiftWheel", "mouse", "mouse"]],
	},
	{
		name: "two confident trackpad gestures switch a mouse session, and the third pans",
		gestures: [["notch100", "mouse", "mouse"], ["trackpadSwipe", "mouse", "mouse"], ["trackpadSwipe", "mouse", "trackpad"], ["trackpadSwipe", "trackpad", "trackpad"]],
	},
	{
		name: "two confident mouse gestures switch a trackpad session",
		gestures: [["trackpadSwipe", "trackpad", "trackpad"], ["notch100", "mouse", "trackpad"], ["lineMode", "mouse", "mouse"]],
	},
	{
		name: "a gesture of the current device between two of the other starts the count again",
		gestures: [["notch100", "mouse", "mouse"], ["magicMouse", "mouse", "mouse"], ["notch120", "mouse", "mouse"], ["magicMouse", "mouse", "mouse"]],
	},
	{
		name: "a flick keeps its verdict through notch-sized steps",
		gestures: [["trackpadSwipe", "trackpad", "trackpad"], ["trackpadFlick", "trackpad", "trackpad"], ["trackpadFlick", "trackpad", "trackpad"]],
	},
];

describe("input profile", () => {
	it.each(cases)("$name", ({ gestures }) => {
		const profile = new InputProfile();
		let now = 1000;
		const seen: [TraceName, InputDevice[], InputDevice | null][] = [];
		for (const [name] of gestures) {
			const trace: Trace = traces[name];
			// Gestures are separated by a pause well over the 150 ms that ends one.
			now += 400;
			const devices = new Set<InputDevice>();
			trace.steps.forEach((step, index) => {
				now += trace.everyMs;
				const verdict = profile.wheel({ ...step, timeStamp: now });
				expect(verdict.gestureStart).toBe(index === 0);
				devices.add(verdict.device);
			});
			seen.push([name, [...devices], profile.detected]);
		}
		expect(seen).toEqual(gestures.map(([name, actsAs, detectedAfter]) => [name, [actsAs], detectedAfter]));
	});
});
