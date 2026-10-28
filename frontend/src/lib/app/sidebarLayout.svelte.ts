import { tick } from "svelte";
import { focusTrapTarget } from "../focusTrap";

export const TAG_PANEL_MIN_WIDTH_PX = 260;
export const TAG_PANEL_MAX_WIDTH_PX = 720;
const TAG_PANEL_DEFAULT_WIDTH_PX = 420;
const TAG_PANEL_COLLAPSED_WIDTH_PX = 44;
export const FILE_NAV_MIN_WIDTH_PX = 200;
export const FILE_NAV_MAX_WIDTH_PX = 560;
const FILE_NAV_DEFAULT_WIDTH_PX = 276;
const FILE_NAV_COLLAPSED_WIDTH_PX = 44;
/** Dragging a sidebar wider stops where the image would get narrower than this. */
const VIEWER_MIN_WIDTH_PX = 320;
/** Arrow-key step on a focused sidebar handle. */
const KEYBOARD_STEP_PX = 16;
/** Widths at which each sidebar becomes a compact drawer (matches App's CSS). */
const EXPLORER_DRAWER_QUERY = "(max-width: 519px)";
const TAGS_DRAWER_QUERY = "(max-width: 979px)";
const DRAWER_FOCUSABLE_SELECTOR = [
	"a[href]",
	"button:not([disabled])",
	"input:not([disabled])",
	"select:not([disabled])",
	"textarea:not([disabled])",
	"[tabindex]:not([tabindex=\"-1\"])",
].join(",");

export type CompactDrawer = "explorer" | "tags";
export type Sidebar = "explorer" | "tags";

type ResizeDrag = { sidebar: Sidebar; pointerId: number; startX: number; startWidth: number };

/**
 * Clamps a sidebar's width to its own range and, when the window is known,
 * to what leaves the image at least `VIEWER_MIN_WIDTH_PX` beside the other
 * sidebar. The sidebar's minimum always wins over the image's.
 */
export function clampSidebarWidth(sidebar: Sidebar, width: number, otherSidebarPx = 0, windowWidthPx = Infinity): number {
	const [min, max] = sidebar === "explorer"
		? [FILE_NAV_MIN_WIDTH_PX, FILE_NAV_MAX_WIDTH_PX]
		: [TAG_PANEL_MIN_WIDTH_PX, TAG_PANEL_MAX_WIDTH_PX];
	const room = windowWidthPx - otherSidebarPx - VIEWER_MIN_WIDTH_PX;
	return Math.max(min, Math.min(max, room, width));
}

/**
 * The explorer and tag-panel sidebars: collapse state, their resizable
 * widths, and, on narrow screens, the modal drawer either sidebar
 * turns into (focus moves in on open, is trapped while open, and returns to
 * its toggle button on close).
 */
export class SidebarLayout {
	fileNavigatorCollapsed = $state(false);
	tagPanelCollapsed = $state(false);
	tagPanelWidthPx = $state(TAG_PANEL_DEFAULT_WIDTH_PX);
	fileNavigatorExpandedWidthPx = $state(FILE_NAV_DEFAULT_WIDTH_PX);
	compactDrawer = $state<CompactDrawer | null>(null);
	resizing = $state<ResizeDrag | null>(null);
	/** Bound by App to the drawer toggle buttons and drawer containers. */
	explorerButton = $state<HTMLButtonElement | null>(null);
	tagsButton = $state<HTMLButtonElement | null>(null);
	explorerDrawer = $state<HTMLElement | null>(null);
	tagsDrawer = $state<HTMLElement | null>(null);

	readonly fileNavigatorWidthPx = $derived(this.fileNavigatorCollapsed ? FILE_NAV_COLLAPSED_WIDTH_PX : this.fileNavigatorExpandedWidthPx);
	readonly tagPanelWidth = $derived(this.tagPanelCollapsed ? TAG_PANEL_COLLAPSED_WIDTH_PX : this.tagPanelWidthPx);

	toggleTagPanel(): void {
		this.tagPanelCollapsed = !this.tagPanelCollapsed;
	}

	toggleDrawer(drawer: CompactDrawer): void {
		if (this.compactDrawer === drawer) {
			this.closeDrawer(false);
			return;
		}
		if (drawer === "explorer") {
			this.fileNavigatorCollapsed = false;
		} else {
			this.tagPanelCollapsed = false;
		}
		this.compactDrawer = drawer;
		void tick().then(() => this.#drawerElement(drawer)?.focus());
	}

	closeDrawer(restoreFocus = true): void {
		const closing = this.compactDrawer;
		if (closing === null) return;
		this.compactDrawer = null;
		if (restoreFocus) {
			void tick().then(() => (closing === "explorer" ? this.explorerButton : this.tagsButton)?.focus());
		}
	}

	/** Opening a file from the explorer drawer closes it. */
	fileOpenedFromExplorer(): void {
		if (window.matchMedia(EXPLORER_DRAWER_QUERY).matches) this.closeDrawer();
	}

	/** A drawer whose sidebar is no longer compact at the new width closes. */
	viewportResized(): void {
		if (this.compactDrawer === "explorer" && !window.matchMedia(EXPLORER_DRAWER_QUERY).matches) {
			this.closeDrawer(false);
		}
		if (this.compactDrawer === "tags" && !window.matchMedia(TAGS_DRAWER_QUERY).matches) {
			this.closeDrawer(false);
		}
	}

	/** Keeps Tab focus inside the open drawer. */
	trapDrawerFocus(event: KeyboardEvent): void {
		if (event.key !== "Tab" || this.compactDrawer === null) return;
		const container = this.#drawerElement(this.compactDrawer);
		if (!container) return;
		const focusable = Array.from(
			container.querySelectorAll<HTMLElement>(DRAWER_FOCUSABLE_SELECTOR),
		).filter((element) => element.getClientRects().length > 0 && getComputedStyle(element).visibility !== "hidden");
		const activeIndex = focusable.indexOf(document.activeElement as HTMLElement);
		const target = focusTrapTarget(activeIndex, focusable.length, event.shiftKey);
		if (target === null) return;
		event.preventDefault();
		if (target === "container") {
			container.focus();
		} else if (target === "first") {
			focusable[0]?.focus();
		} else {
			focusable[focusable.length - 1]?.focus();
		}
	}

	startResize(sidebar: Sidebar, event: PointerEvent): void {
		if (this.#collapsed(sidebar) || event.button !== 0) return;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		this.resizing = { sidebar, pointerId: event.pointerId, startX: event.clientX, startWidth: this.#width(sidebar) };
		event.preventDefault();
	}

	moveResize(event: PointerEvent): void {
		const drag = this.resizing;
		if (!drag || drag.pointerId !== event.pointerId) return;
		// The explorer's handle is on its right edge, the tag panel's on its
		// left, so dragging away from the image widens either one.
		const delta = event.clientX - drag.startX;
		this.#setWidth(drag.sidebar, drag.startWidth + (drag.sidebar === "explorer" ? delta : -delta));
	}

	endResize(event: PointerEvent): void {
		const handle = event.currentTarget as HTMLElement;
		if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
		if (this.resizing?.pointerId === event.pointerId) this.resizing = null;
	}

	cancelResize(): void {
		this.resizing = null;
	}

	/** Arrow keys on a focused handle move it by a step; Home and End go to the limits. */
	resizeWithKeyboard(sidebar: Sidebar, event: KeyboardEvent): void {
		if (this.#collapsed(sidebar)) return;
		const outward = sidebar === "explorer" ? "ArrowRight" : "ArrowLeft";
		const inward = sidebar === "explorer" ? "ArrowLeft" : "ArrowRight";
		const width = this.#width(sidebar);
		let next: number;
		if (event.key === outward) next = width + KEYBOARD_STEP_PX;
		else if (event.key === inward) next = width - KEYBOARD_STEP_PX;
		else if (event.key === "Home") next = 0;
		else if (event.key === "End") next = Infinity;
		else return;
		event.preventDefault();
		event.stopPropagation();
		this.#setWidth(sidebar, next);
	}

	#collapsed(sidebar: Sidebar): boolean {
		return sidebar === "explorer" ? this.fileNavigatorCollapsed : this.tagPanelCollapsed;
	}

	#width(sidebar: Sidebar): number {
		return sidebar === "explorer" ? this.fileNavigatorExpandedWidthPx : this.tagPanelWidthPx;
	}

	#setWidth(sidebar: Sidebar, width: number): void {
		// Below the drawer width the tag panel floats over the image and takes no room.
		const other = sidebar === "tags"
			? this.fileNavigatorWidthPx
			: window.matchMedia(TAGS_DRAWER_QUERY).matches ? 0 : this.tagPanelWidth;
		const clamped = clampSidebarWidth(sidebar, width, other, window.innerWidth);
		if (sidebar === "explorer") this.fileNavigatorExpandedWidthPx = clamped;
		else this.tagPanelWidthPx = clamped;
	}

	#drawerElement(drawer: CompactDrawer): HTMLElement | null {
		return drawer === "explorer" ? this.explorerDrawer : this.tagsDrawer;
	}
}
