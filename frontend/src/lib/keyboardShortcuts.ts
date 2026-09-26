import { TOOL_ORDER, TOOL_SHORTCUTS, type ActiveTool } from "./viewerTools";

/** What a global key press asks the viewer to do. */
export type ShortcutAction =
	| { type: "close-drawer" }
	| { type: "select-adjacent-file"; step: -1 | 1 }
	| { type: "select-tool"; tool: ActiveTool }
	| { type: "step-frame"; step: -1 | 1 }
	| { type: "toggle-cine" }
	| { type: "delete-roi" };

export type ShortcutContext = {
	/** A compact sidebar drawer is open. */
	drawerOpen: boolean;
	/** The open tab has more than one logical frame. */
	multiFrame: boolean;
	/** The ROI tool is active on a pixel viewport. */
	roiToolActive: boolean;
};

export type ShortcutKeyEvent = Pick<KeyboardEvent, "key" | "target" | "altKey" | "ctrlKey" | "metaKey" | "shiftKey">;

const TOOL_KEYS = new Map<string, ActiveTool>(
	TOOL_ORDER.map((tool) => [TOOL_SHORTCUTS[tool].toLowerCase(), tool]),
);

/** Text entry and form controls keep their keys; shortcuts never fire there. */
export function isEditableTarget(target: EventTarget | null): boolean {
	const element = target as Partial<HTMLElement> | null;
	if (!element || typeof element.tagName !== "string") return false;
	return element.isContentEditable === true || ["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName);
}

/**
 * Maps a window keydown to a viewer action. Escape closes an open drawer
 * from anywhere; every other shortcut is ignored while focus is in an
 * editable control.
 */
export function shortcutFor(event: ShortcutKeyEvent, context: ShortcutContext): ShortcutAction | null {
	if (event.key === "Escape" && context.drawerOpen) return { type: "close-drawer" };
	if (isEditableTarget(event.target)) return null;

	const modified = event.altKey || event.ctrlKey || event.metaKey || event.shiftKey;
	if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !modified) {
		return { type: "select-adjacent-file", step: event.key === "ArrowUp" ? -1 : 1 };
	}
	const tool = TOOL_KEYS.get(event.key.toLowerCase());
	if (tool) return { type: "select-tool", tool };
	if (context.multiFrame) {
		if (event.key === "ArrowLeft" || event.key === "[") return { type: "step-frame", step: -1 };
		if (event.key === "ArrowRight" || event.key === "]") return { type: "step-frame", step: 1 };
		if (event.key === " ") return { type: "toggle-cine" };
	}
	if (context.roiToolActive && (event.key === "Delete" || event.key === "Backspace")) {
		return { type: "delete-roi" };
	}
	return null;
}
