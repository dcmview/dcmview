import type { EmbedRoiAnnotations, FileSummary } from "../../../api";
import type { ImagePoint } from "../../annotationGeometry";
import type { ActiveTool } from "../../viewerTools";
import type { VisibleRoi } from "../../viewport/roiEditing";
import type { ViewTransform, ZoomAnchor } from "../../viewport/viewTransform";

export type ToolId = ActiveTool;

/** A pointer position in client (CSS pixel) coordinates. */
export type ToolPointer = { clientX: number; clientY: number };

/** A wheel step in CSS pixels, whatever unit the device reported. */
export type ToolWheel = { dx: number; dy: number };

/** The frames the viewport steps through: a file's frames or a stack's images. */
export interface FrameNavigation {
	readonly count: number;
	readonly position: number;
	/** Shows the image at `position` and stops cine playback. */
	go(position: number): void;
}

/** The window a window/level drag starts from; `step` is one unit of drag travel. */
export type WindowDragBase = { center: number; width: number; step: number };

/** The live window of a window/level drag. The host ends it with the gesture. */
export interface WindowLevelSession {
	/** Starts a live window at the displayed one; null when this frame cannot be windowed now. */
	begin(): WindowDragBase | null;
	preview(center: number, width: number): void;
	/** Reports the previewed window as the user's manual window, if it is still live. */
	commit(): void;
}

/** The rectangles the rectangle tools edit on the active file: ROIs, or redaction boxes. */
export interface RectangleEdits {
	/** False until the file's rectangles have loaded and the displayed frame is the requested one. */
	readonly editable: boolean;
	/** The rectangles drawn on the displayed frame. */
	readonly visible: readonly VisibleRoi[];
	readonly annotations: EmbedRoiAnnotations | null;
	readonly selectedIndex: number | null;
	/** A new rectangle covers every frame (a redaction box) and not just the one it is drawn on. */
	readonly coversAllFrames: boolean;
	select(index: number | null): void;
	/** Keeps save completions from replacing the geometry under the pointer. */
	beginLiveEdit(): void;
	/** Shows geometry of `fileIndex` that is not saved yet. */
	showDraft(fileIndex: number, annotations: EmbedRoiAnnotations): void;
	/** Shows and saves the active file's rectangles, selecting `selectedIndex`. */
	commit(annotations: EmbedRoiAnnotations, selectedIndex: number | null): void;
}

/** The opposite corners of a rectangle being drawn, in image coordinates. */
export type DraftRect = { start: ImagePoint; current: ImagePoint };

/**
 * What a tool sees of the viewport. Members are read live: a gesture that
 * outlasts a file, frame or tool change acts on what is shown at that moment.
 */
export interface ToolContext {
	readonly file: FileSummary;
	/** The requested frame of `file`. */
	readonly frame: number;
	readonly imageRows: number;
	readonly imageColumns: number;
	readonly transform: ViewTransform;
	setTransform(transform: Omit<ViewTransform, "fit">): void;
	/** Image coordinates under a client point, clamped to the image; null before layout. */
	toImage(clientX: number, clientY: number): ImagePoint | null;
	/** Pins the image point under a client point for zooming; null before layout. */
	zoomAnchor(clientX: number, clientY: number): ZoomAnchor | null;
	/** The transform at `scale` that keeps `anchor` under its client point; null before layout. */
	zoomTransform(scale: number, anchor: ZoomAnchor): Omit<ViewTransform, "fit"> | null;
	readonly navigation: FrameNavigation;
	readonly window: WindowLevelSession;
	readonly rects: RectangleEdits;
}

/**
 * One tool's gestures as a state machine. The host owns pointer capture and
 * the universal gestures, and calls `pointerMove`, `pointerUp` and `cancel`
 * only for the tool whose `pointerDown` captured the pointer.
 */
export interface Tool {
	readonly id: ToolId;
	/**
	 * A gesture of this tool belongs to the file and frame it began on; the
	 * host cancels it when another one is shown.
	 */
	readonly frameBound: boolean;
	/** What the gesture in progress is drawing; the host shows it while this tool holds the pointer. */
	readonly draft?: DraftRect | null;
	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" | "ignore";
	pointerMove(pointer: ToolPointer, ctx: ToolContext): void;
	/** Commits the gesture in progress. */
	pointerUp(ctx: ToolContext): void;
	/** A wheel step while this tool is active; true when the tool used it. */
	wheel?(wheel: ToolWheel, ctx: ToolContext): boolean;
	/** Undoes what the gesture in progress has shown without saving it. */
	cancel(ctx: ToolContext): void;
	/** Drops the gesture in progress, neither committing nor undoing it. */
	reset(): void;
}
