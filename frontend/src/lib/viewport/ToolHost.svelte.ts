import { PanTool } from "../annotation/tools/panTool";
import { RectangleTool } from "../annotation/tools/rectangleTool";
import { ScrollTool } from "../annotation/tools/scrollTool";
import type { DraftRect, Tool, ToolContext, ToolId, ToolPointer, ToolWheel } from "../annotation/tools/tool";
import { WindowLevelTool } from "../annotation/tools/windowLevelTool";
import { ZoomTool } from "../annotation/tools/zoomTool";
import { InputProfile, type InputDevice, type InputProfileSetting, type WheelVerdict } from "./inputProfile";

const MOUSE_WHEEL_ZOOM_SENSITIVITY = 0.0025;
const PINCH_ZOOM_SENSITIVITY = 0.01;

/** What the host needs of the viewport beyond what it hands to tools. */
export interface ToolHostView extends ToolContext {
	readonly activeTool: ToolId;
	/** The frame on screen is the requested frame of the active file. */
	readonly displayedFrameIsCurrent: boolean;
	readonly viewportHeight: number;
	/** Zooms to `scale` keeping the image point under a client point in place. */
	zoomAt(scale: number, clientX: number, clientY: number): void;
	/** Moves the pixel readout to a client point. */
	scheduleProbe(clientX: number, clientY: number): void;
	/** A gesture ended, or there was none: drop the live window it was showing. */
	gestureEnded(): void;
}

/** Controls drawn over the image keep their own pointer and wheel events. */
export function isViewportChromeTarget(target: EventTarget | null): boolean {
	return target instanceof Element && !!target.closest(".zoom-controls, .roi-list");
}

function toolPointer(event: PointerEvent): ToolPointer {
	return { clientX: event.clientX, clientY: event.clientY };
}

/**
 * Routes the viewport's pointer and wheel events: it owns pointer capture,
 * the gestures every tool shares (middle-button pan, right-button zoom,
 * wheel pan and zoom, pinch), the input profile that tells a mouse wheel from a trackpad, the
 * cancel of a gesture whose file or frame was replaced, and the tool that
 * holds the pointer or is armed between the two clicks of a placement. The
 * tools are state machines behind `Tool`.
 */
export class ToolHost {
	readonly #view: ToolHostView;
	readonly #tools: Record<ToolId, Tool>;
	readonly #scroll = new ScrollTool();
	#captured = $state.raw<Tool | null>(null);
	// The pointer whose press began the gesture; other pointers are ignored until it ends.
	#pointerId = 0;
	// The tool whose gesture goes on between presses (click-click placement).
	#armed = $state.raw<Tool | null>(null);
	// The element holding the pointer capture.
	#surface: HTMLElement | null = null;
	#draft = $state.raw<DraftRect | null>(null);
	// The frame-bound gesture begun since the last one ended, and where it began.
	#frameGesture: { tool: Tool; fileIndex: number; frameIndex: number } | null = null;
	// Wheel events are told apart here and nowhere else (inputProfile.ts).
	readonly #profile = new InputProfile();
	#inputProfile = $state.raw<InputDevice>("mouse");
	#inputProfileSetting = $state.raw<InputProfileSetting>("auto");

	constructor(view: ToolHostView) {
		this.#view = view;
		this.#tools = {
			pan: new PanTool(),
			scroll: this.#scroll,
			zoom: new ZoomTool(),
			window_level: new WindowLevelTool(),
			annotate_rect: new RectangleTool("annotate_rect"),
			redact: new RectangleTool("redact"),
		};
	}

	/** A tool holds the pointer. */
	get dragging(): boolean {
		return this.#captured !== null;
	}

	/** The tool holding the pointer, which stays the same if the active tool changes mid-gesture. */
	get capturedTool(): ToolId | null {
		return this.#captured?.id ?? null;
	}

	/** The rectangle being drawn, by the tool holding the pointer or by an armed tool between its clicks. */
	get draft(): DraftRect | null {
		// An armed tool's rectangle belongs to the tool and frame it began on:
		// it is not drawn over another, even before `shownChanged` drops it.
		if (this.#armed && (this.#armed.id !== this.#view.activeTool || this.#frameGestureReplaced())) return null;
		return this.#draft;
	}

	/** The device the session is taken to use: what the wheel has shown, or the override. */
	get inputProfile(): InputDevice {
		return this.#inputProfile;
	}

	/** Auto follows the wheel; Mouse or Trackpad overrides it. Kept for the page, like other view state. */
	get inputProfileSetting(): InputProfileSetting {
		return this.#inputProfileSetting;
	}

	set inputProfileSetting(setting: InputProfileSetting) {
		this.#profile.setting = setting;
		this.#inputProfileSetting = setting;
		this.#inputProfile = this.#profile.device;
	}

	wheel(event: WheelEvent): void {
		const view = this.#view;
		if (!view.file.has_pixels) return;
		if (isViewportChromeTarget(event.target)) return;
		event.preventDefault();

		const { dx, dy } = this.#wheelDeltaPixels(event);
		const { device, gestureStart } = this.#classifyWheel(event);
		const wheel: ToolWheel = { dx, dy, device, gestureStart };
		// Ctrl or Meta is how browsers report a pinch, which zooms in every tool.
		if (event.ctrlKey || event.metaKey) {
			this.#zoomByWheelDelta(dy, event.clientX, event.clientY, PINCH_ZOOM_SENSITIVITY);
			return;
		}
		// Alt+wheel steps frames in every tool, and does nothing else on a single image.
		if (event.altKey) {
			this.#scroll.wheel(wheel, view);
			return;
		}
		if (this.#tools[view.activeTool].wheel?.(wheel, view)) return;
		// Two fingers pan; so does a wheel that only moves sideways (a tilt wheel, Shift+wheel).
		if (device === "trackpad" || dy === 0) {
			view.setTransform({
				...view.transform,
				tx: view.transform.tx - dx,
				ty: view.transform.ty - dy,
			});
			return;
		}
		this.#zoomByWheelDelta(dy, event.clientX, event.clientY, MOUSE_WHEEL_ZOOM_SENSITIVITY);
		view.scheduleProbe(event.clientX, event.clientY);
	}

	pointerDown(event: PointerEvent): void {
		const view = this.#view;
		if (!view.file.has_pixels) return;
		if (isViewportChromeTarget(event.target)) return;
		// One gesture at a time: a press by another pointer must not take over
		// the tools, or the gesture in progress would end neither saved nor undone.
		if (this.#captured) {
			if (event.button !== 0) event.preventDefault();
			return;
		}
		// A placement left behind by a tool, file or frame change does not take this press.
		if (this.#liveArmed()) this.#cancelReplacedFrameGesture();

		if (event.button === 1) {
			event.preventDefault();
			this.#begin(this.#tools.pan, event);
			return;
		}

		// A right-drag zooms in every tool; a right click that does not move changes nothing.
		if (event.button === 2) {
			event.preventDefault();
			this.#begin(this.#tools.zoom, event);
			return;
		}

		if (event.button === 0) {
			this.#begin(this.#tools[view.activeTool], event);
		}
	}

	pointerMove(event: PointerEvent): void {
		if (this.#captured && event.pointerId !== this.#pointerId) return;
		// With no button held, the moves go to a tool armed between its clicks.
		const armed = this.#liveArmed();
		const tool = this.#captured ?? armed;
		if (!tool) return;
		if (this.#cancelReplacedFrameGesture()) return;
		tool.pointerMove(toolPointer(event), this.#view);
		this.#draft = (armed ?? tool).draft ?? null;
	}

	pointerUp(event: PointerEvent): void {
		if (this.#captured && event.pointerId !== this.#pointerId) return;
		const target = event.currentTarget as HTMLElement;
		if (target.hasPointerCapture(event.pointerId)) {
			target.releasePointerCapture(event.pointerId);
		}
		if (this.#cancelReplacedFrameGesture()) return;
		const tool = this.#captured;
		if (!tool) {
			if (!this.#armed) this.endGesture();
			return;
		}
		tool.pointerUp(this.#view);
		this.#captured = null;
		// The release left the tool armed: its gesture goes on with no button held.
		if (tool.armed) {
			this.#armed = tool;
			this.#draft = tool.draft ?? null;
			return;
		}
		// A middle- or right-button drag over an armed tool ends alone.
		if (this.#armed && this.#armed !== tool) {
			tool.reset();
			return;
		}
		this.endGesture();
	}

	/** The browser took the pointer away; without an event, cancels whatever is in progress. */
	pointerCancel(event?: PointerEvent): void {
		if (event && (this.#captured ? event.pointerId !== this.#pointerId : this.#armed !== null)) return;
		const frameTool = this.#frameGesture?.tool;
		if (frameTool && frameTool !== this.#captured) frameTool.cancel(this.#view);
		this.#captured?.cancel(this.#view);
		this.endGesture();
	}

	/** The viewport shows another tool, file or frame: a placement begun on the last one ends. */
	shownChanged(): void {
		if (this.#captured || !this.#liveArmed()) return;
		if (this.#frameGestureReplaced()) this.pointerCancel();
	}

	/**
	 * Escape: cancels a click-click placement between or during its clicks.
	 * False when there is none, so the key keeps its other meanings.
	 */
	cancelPlacement(): boolean {
		if (!this.#liveArmed()) return false;
		if (this.#captured && this.#surface?.hasPointerCapture(this.#pointerId)) {
			this.#surface.releasePointerCapture(this.#pointerId);
		}
		this.pointerCancel();
		return true;
	}

	/** Drops the gesture in progress without committing or undoing it. */
	endGesture(): void {
		this.#view.gestureEnded();
		for (const tool of Object.values(this.#tools)) tool.reset();
		this.#captured = null;
		this.#armed = null;
		this.#draft = null;
		this.#frameGesture = null;
	}

	#begin(tool: Tool, event: PointerEvent): void {
		const view = this.#view;
		if (tool.pointerDown(toolPointer(event), view) !== "capture") return;
		event.preventDefault();
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		this.#captured = tool;
		this.#pointerId = event.pointerId;
		this.#surface = event.currentTarget as HTMLElement;
		this.#draft = (this.#armed ?? tool).draft ?? null;
		if (tool.frameBound) this.#frameGesture = { tool, fileIndex: view.file.index, frameIndex: view.frame };
	}

	#cancelReplacedFrameGesture(): boolean {
		if (!this.#frameGesture || (this.#view.displayedFrameIsCurrent && !this.#frameGestureReplaced())) return false;
		this.pointerCancel();
		return true;
	}

	/** Another file or frame is requested than the one the frame-bound gesture began on. */
	#frameGestureReplaced(): boolean {
		const began = this.#frameGesture;
		return began !== null && (began.fileIndex !== this.#view.file.index || began.frameIndex !== this.#view.frame);
	}

	/** The armed tool, unless another tool was chosen since: then its gesture is dropped. */
	#liveArmed(): Tool | null {
		const armed = this.#armed;
		if (!armed || this.#captured || this.#tools[this.#view.activeTool] === armed) return armed;
		armed.cancel(this.#view);
		this.endGesture();
		return null;
	}

	/** The one place a wheel event is told apart: the device its gesture acts as. */
	#classifyWheel(event: WheelEvent): WheelVerdict {
		const { wheelDeltaY } = event as WheelEvent & { wheelDeltaY?: unknown };
		const verdict = this.#profile.wheel({
			deltaX: event.deltaX,
			deltaY: event.deltaY,
			deltaMode: event.deltaMode,
			wheelDeltaY: typeof wheelDeltaY === "number" ? wheelDeltaY : undefined,
			timeStamp: event.timeStamp,
		});
		this.#inputProfile = this.#profile.device;
		return verdict;
	}

	#wheelDeltaPixels(event: WheelEvent): { dx: number; dy: number } {
		if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
			return { dx: event.deltaX * 16, dy: event.deltaY * 16 };
		}
		if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
			const page = this.#view.viewportHeight || window.innerHeight || 800;
			return { dx: event.deltaX * page, dy: event.deltaY * page };
		}
		return { dx: event.deltaX, dy: event.deltaY };
	}

	#zoomByWheelDelta(deltaY: number, clientX: number, clientY: number, sensitivity: number): void {
		if (deltaY === 0) return;
		this.#view.zoomAt(this.#view.transform.scale * Math.exp(-deltaY * sensitivity), clientX, clientY);
	}
}
