/**
 * Tells a mouse from a trackpad by its wheel events, the only evidence a
 * browser gives (both report `pointerType: "mouse"`). Implements
 * docs/design/annotation-tools-ux.md section 3.5: a gesture is classified
 * once and keeps its verdict, and the session profile follows gestures with
 * hysteresis. Pure: no DOM, no clock; callers pass each event's time stamp.
 */

export type InputDevice = "mouse" | "trackpad";

/** The owner's choice: follow the wheel evidence, or always act as one device. */
export type InputProfileSetting = "auto" | InputDevice;

/** What the classifier reads of a wheel event. */
export type WheelSample = {
	deltaX: number;
	deltaY: number;
	/** `WheelEvent.deltaMode`: 0 pixels, 1 lines, 2 pages. */
	deltaMode: number;
	/** The legacy `WheelEvent.wheelDeltaY`, where the browser has it: 120 per wheel notch. */
	wheelDeltaY?: number;
	/** `Event.timeStamp`, in milliseconds. */
	timeStamp: number;
};

/** The device one wheel event acts as. */
export type WheelVerdict = {
	device: InputDevice;
	/** This event begins a gesture: the first after a pause. */
	gestureStart: boolean;
};

/** Wheel events closer together than this are one gesture. */
export const GESTURE_GAP_MS = 150;
/** A gesture is judged by its first events only. */
const EVIDENCE_EVENTS = 4;
/** This many confident gestures of the other device in a row switch the profile. */
const GESTURES_TO_SWITCH = 2;
/** The pixel step under which the old per-event rule took a wheel for a trackpad. */
const SMALL_PIXEL_STEP = 50;
/** A wheel that reports fractions of a notch still steps by at least this much. */
const MIN_WHEEL_QUANTUM = 4;

const PIXEL_MODE = 0;

/** One or more whole notches of a wheel that reports 100 or 120 pixels per notch. */
function isNotch(delta: number): boolean {
	const size = Math.abs(delta);
	return size >= 100 && (size % 100 === 0 || size % 120 === 0);
}

function isMultipleOf(value: number, step: number): boolean {
	const ratio = value / step;
	return Math.abs(ratio - Math.round(ratio)) <= 0.01 * ratio;
}

/**
 * `wheelDeltaY` counts 120 per wheel notch whatever `deltaY` says, and three
 * per pixel for a trackpad. A whole number of notches that is not three times
 * the pixel delta is a wheel, including one that reports a few pixels a notch.
 */
function hasNotchedWheelDelta(sample: WheelSample): boolean {
	const legacy = sample.wheelDeltaY;
	if (legacy === undefined || legacy === 0 || legacy % 120 !== 0) return false;
	return Math.abs(legacy + 3 * sample.deltaY) > 2;
}

/**
 * The device the first events of a gesture came from, or null while they
 * could be either. `samples` are the gesture's events in order, without
 * those that moved nothing.
 */
export function gestureEvidence(samples: readonly WheelSample[]): InputDevice | null {
	if (samples.length === 0) return null;
	// Only wheels report lines or pages.
	if (samples.some((sample) => sample.deltaMode !== PIXEL_MODE)) return "mouse";
	if (samples.some(hasNotchedWheelDelta)) return "mouse";
	// A wheel moves along one axis; a tilt wheel or Shift+wheel sends whole notches sideways.
	if (samples.some((sample) => sample.deltaX !== 0 && !(sample.deltaY === 0 && isNotch(sample.deltaX)))) {
		return "trackpad";
	}
	if (samples.some((sample) => sample.deltaX !== 0)) return null;
	const sizes = samples.map((sample) => Math.abs(sample.deltaY));
	if (sizes.every(isNotch)) return "mouse";
	// A free-spinning or high-resolution wheel sends small pixel steps, each a
	// multiple of one quantum; two fingers send whatever they travelled.
	const quantum = Math.min(...sizes);
	const quantized = quantum >= MIN_WHEEL_QUANTUM && sizes.every((size) => isMultipleOf(size, quantum));
	if (quantized) return samples.length >= EVIDENCE_EVENTS ? "mouse" : null;
	return samples.length >= 3 ? "trackpad" : null;
}

/** The per-event rule the viewer used before it kept a profile; it decides while nothing is known. */
function deviceOfLoneEvent(sample: WheelSample): InputDevice {
	if (sample.deltaMode !== PIXEL_MODE) return "mouse";
	return sample.deltaX !== 0 || Math.abs(sample.deltaY) < SMALL_PIXEL_STEP ? "trackpad" : "mouse";
}

/**
 * The session's input profile. Feed it every wheel event: it answers with
 * the device the event's gesture acts as, which never changes inside a
 * gesture, and it moves the detected profile after two confident gestures of
 * the other device in a row, so one ambiguous event cannot flip it.
 */
export class InputProfile {
	/** Mouse or Trackpad overrides what the wheel says. */
	setting: InputProfileSetting = "auto";
	#detected: InputDevice | null = null;
	#contrary = 0;
	#gesture: { device: InputDevice; last: number; samples: WheelSample[]; counted: boolean } | null = null;

	/** What the wheel events have shown so far; null before the first confident gesture. */
	get detected(): InputDevice | null {
		return this.#detected;
	}

	/** The profile in force. A session that has shown nothing yet is taken for a mouse. */
	get device(): InputDevice {
		return this.setting === "auto" ? this.#detected ?? "mouse" : this.setting;
	}

	wheel(sample: WheelSample): WheelVerdict {
		const previous = this.#gesture;
		const elapsed = previous ? sample.timeStamp - previous.last : Infinity;
		const gestureStart = !previous || elapsed >= GESTURE_GAP_MS || elapsed < 0;
		// The profile as it stood before this gesture said anything.
		const known = this.#detected;
		const gesture = gestureStart || !previous
			? { device: "mouse" as InputDevice, last: sample.timeStamp, samples: [], counted: false }
			: previous;
		gesture.last = sample.timeStamp;
		this.#gesture = gesture;

		let evidence: InputDevice | null = null;
		const moved = sample.deltaX !== 0 || sample.deltaY !== 0;
		if (moved && !gesture.counted && gesture.samples.length < EVIDENCE_EVENTS) {
			gesture.samples.push(sample);
			evidence = gestureEvidence(gesture.samples);
			if (evidence) {
				gesture.counted = true;
				this.#count(evidence);
			}
		}
		if (gestureStart) gesture.device = evidence ?? known ?? deviceOfLoneEvent(sample);
		return { device: this.setting === "auto" ? gesture.device : this.setting, gestureStart };
	}

	#count(device: InputDevice): void {
		if (this.#detected === null || this.#detected === device) {
			this.#detected = device;
			this.#contrary = 0;
			return;
		}
		this.#contrary += 1;
		if (this.#contrary >= GESTURES_TO_SWITCH) {
			this.#detected = device;
			this.#contrary = 0;
		}
	}
}
