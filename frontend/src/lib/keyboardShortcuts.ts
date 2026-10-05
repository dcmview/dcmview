import { TOOL_ORDER, TOOL_SHORTCUTS, type ActiveTool } from "./viewerTools";

/** What a global key press asks the viewer to do. */
export type ShortcutAction =
	| { type: "close-drawer" }
	| { type: "select-adjacent-file"; step: -1 | 1 }
	| { type: "select-tool"; tool: ActiveTool }
	| { type: "step-frame"; step: -1 | 1 }
	| { type: "toggle-cine" }
	| { type: "step-annotation-item"; step: -1 | 1 }
	| { type: "delete-roi" };

export type ShortcutContext = {
	/** A compact sidebar drawer is open. */
	drawerOpen: boolean;
	/** The open tab has more than one logical frame. */
	multiFrame: boolean;
	/** The ROI tool is active on a pixel viewport. */
	roiToolActive: boolean;
	/** A presentation state with annotation items is shown. */
	annotationItems: boolean;
};

export type ShortcutKeyEvent = Pick<KeyboardEvent, "key" | "target" | "altKey" | "ctrlKey" | "metaKey" | "shiftKey">;

const TOOL_KEYS = new Map<string, ActiveTool>(
	TOOL_ORDER.map((tool) => [TOOL_SHORTCUTS[tool].toLowerCase(), tool]),
);

function isRangeTarget(target: EventTarget | null): boolean {
	const element = target as Partial<HTMLInputElement> | null;
	return element?.tagName === "INPUT" && element.type === "range";
}

/** Text entry and select controls keep their keys. */
export function isEditableTarget(target: EventTarget | null): boolean {
	const element = target as Partial<HTMLElement> | null;
	if (!element || typeof element.tagName !== "string") return false;
	return element.isContentEditable === true || !isRangeTarget(target) && ["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName);
}

/**
 * Ctrl, Meta or Alt is held. No binding names one of them, so the key is the
 * browser's, the system's or VS Code's (Ctrl+Z, Ctrl+R, Cmd+[) and not ours.
 */
function hasCommandModifier(event: ShortcutKeyEvent): boolean {
	return event.altKey || event.ctrlKey || event.metaKey;
}

/**
 * The key may have been typed with AltGr, which Windows reports as Ctrl+Alt,
 * or with Option. Layouts without bracket keys type `[` and `]` that way.
 */
function mayBeTypedWithAltGraph(event: ShortcutKeyEvent): boolean {
	return event.altKey && !event.metaKey;
}

/**
 * Maps a window keydown to a viewer action. Escape closes an open drawer
 * from anywhere; every other shortcut is ignored while focus is in an
 * editable control, and while Ctrl, Meta or Alt is held, so those
 * combinations stay with the browser or VS Code. Delete and Backspace are
 * the exception and remove the selected ROI with or without a modifier. Shift alone does not stop a
 * shortcut, except for the file arrows.
 */
export function shortcutFor(event: ShortcutKeyEvent, context: ShortcutContext): ShortcutAction | null {
	if (event.key === "Escape" && context.drawerOpen) return { type: "close-drawer" };
	if (isEditableTarget(event.target)) return null;
	if (isRangeTarget(event.target) && event.key.startsWith("Arrow")) return null;

	if (context.multiFrame && (!hasCommandModifier(event) || mayBeTypedWithAltGraph(event))) {
		if (event.key === "[") return { type: "step-frame", step: -1 };
		if (event.key === "]") return { type: "step-frame", step: 1 };
	}
	// Deleting the selected ROI keeps working with a modifier held:
	// Cmd+Backspace is the usual delete on macOS, not an accidental chord.
	if (context.roiToolActive && (event.key === "Delete" || event.key === "Backspace")) {
		return { type: "delete-roi" };
	}
	if (hasCommandModifier(event)) return null;

	if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !event.shiftKey) {
		return { type: "select-adjacent-file", step: event.key === "ArrowUp" ? -1 : 1 };
	}
	const tool = TOOL_KEYS.get(event.key.toLowerCase());
	if (tool) return { type: "select-tool", tool };
	if (context.multiFrame) {
		if (event.key === "ArrowLeft") return { type: "step-frame", step: -1 };
		if (event.key === "ArrowRight") return { type: "step-frame", step: 1 };
		if (event.key === " ") return { type: "toggle-cine" };
	}
	if (context.annotationItems) {
		if (event.key === ",") return { type: "step-annotation-item", step: -1 };
		if (event.key === ".") return { type: "step-annotation-item", step: 1 };
	}
	return null;
}

/** Shortest gap between auto-repeated steps while a key is held. */
export const REPEAT_INTERVAL_MS = {
	"step-frame": 60,
	"select-adjacent-file": 150,
} as const;

/**
 * Limits how often a held key repeats an action. Each repeat can start
 * fetches and decodes, and the OS repeat rate (~30 Hz) outpaces an uncached
 * stack; a held key still steps, just no faster than the interval. First
 * presses are never throttled.
 */
export class RepeatThrottle {
	readonly #last = new Map<string, number>();

	allow(event: Pick<KeyboardEvent, "key" | "repeat" | "timeStamp">, intervalMs: number): boolean {
		const last = this.#last.get(event.key);
		if (event.repeat && last !== undefined && event.timeStamp - last < intervalMs) return false;
		this.#last.set(event.key, event.timeStamp);
		return true;
	}
}
