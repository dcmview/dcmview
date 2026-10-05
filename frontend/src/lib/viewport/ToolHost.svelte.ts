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
 * holds the pointer. The tools are state machines behind `Tool`.
 */
export class ToolHost {
	readonly #view: ToolHostView;
	readonly #tools: Record<ToolId, Tool>;
	readonly #scroll = new ScrollTool();
	#captured = $state.raw<Tool | null>(null);
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

	/** The rectangle the tool holding the pointer is drawing. */
	get draft(): DraftRect | null {
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
		if (!this.#captured) return;
		if (this.#cancelReplacedFrameGesture()) return;
		this.#captured.pointerMove(toolPointer(event), this.#view);
		this.#draft = this.#captured.draft ?? null;
	}

	pointerUp(event: PointerEvent): void {
		const target = event.currentTarget as HTMLElement;
		if (target.hasPointerCapture(event.pointerId)) {
			target.releasePointerCapture(event.pointerId);
		}
		if (this.#cancelReplacedFrameGesture()) return;
		this.#captured?.pointerUp(this.#view);
		this.endGesture();
	}

	pointerCancel(): void {
		const frameTool = this.#frameGesture?.tool;
		if (frameTool && frameTool !== this.#captured) frameTool.cancel(this.#view);
		this.#captured?.cancel(this.#view);
		this.endGesture();
	}

	/** Drops the gesture in progress without committing or undoing it. */
	endGesture(): void {
		this.#view.gestureEnded();
		for (const tool of Object.values(this.#tools)) tool.reset();
		this.#captured = null;
		this.#draft = null;
		this.#frameGesture = null;
	}

	#begin(tool: Tool, event: PointerEvent): void {
		const view = this.#view;
		if (tool.pointerDown(toolPointer(event), view) !== "capture") return;
		event.preventDefault();
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		this.#captured = tool;
		this.#draft = tool.draft ?? null;
		if (tool.frameBound) this.#frameGesture = { tool, fileIndex: view.file.index, frameIndex: view.frame };
	}

	#cancelReplacedFrameGesture(): boolean {
		const began = this.#frameGesture;
		const view = this.#view;
		if (!began || (view.displayedFrameIsCurrent && began.fileIndex === view.file.index
			&& began.frameIndex === view.frame)) return false;
		this.pointerCancel();
		return true;
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
