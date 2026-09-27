import { tick } from "svelte";
import { focusTrapTarget } from "../focusTrap";

export const TAG_PANEL_MIN_WIDTH_PX = 260;
export const TAG_PANEL_MAX_WIDTH_PX = 720;
const TAG_PANEL_DEFAULT_WIDTH_PX = 420;
const TAG_PANEL_COLLAPSED_WIDTH_PX = 44;
const FILE_NAV_WIDTH_PX = 276;
const FILE_NAV_COLLAPSED_WIDTH_PX = 44;
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

type ResizeDrag = { pointerId: number; startX: number; startWidth: number };

export function clampTagPanelWidth(width: number): number {
	return Math.min(TAG_PANEL_MAX_WIDTH_PX, Math.max(TAG_PANEL_MIN_WIDTH_PX, width));
}

/**
 * The explorer and tag-panel sidebars: collapse state, the resizable tag
 * panel width, and, on narrow screens, the modal drawer either sidebar
 * turns into (focus moves in on open, is trapped while open, and returns to
 * its toggle button on close).
 */
export class SidebarLayout {
	fileNavigatorCollapsed = $state(false);
	tagPanelCollapsed = $state(false);
	tagPanelWidthPx = $state(TAG_PANEL_DEFAULT_WIDTH_PX);
	compactDrawer = $state<CompactDrawer | null>(null);
	resizing = $state<ResizeDrag | null>(null);
	/** Bound by App to the drawer toggle buttons and drawer containers. */
	explorerButton = $state<HTMLButtonElement | null>(null);
	tagsButton = $state<HTMLButtonElement | null>(null);
	explorerDrawer = $state<HTMLElement | null>(null);
	tagsDrawer = $state<HTMLElement | null>(null);

	readonly fileNavigatorWidthPx = $derived(this.fileNavigatorCollapsed ? FILE_NAV_COLLAPSED_WIDTH_PX : FILE_NAV_WIDTH_PX);
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

	startTagPanelResize(event: PointerEvent): void {
		if (this.tagPanelCollapsed || event.button !== 0) return;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		this.resizing = { pointerId: event.pointerId, startX: event.clientX, startWidth: this.tagPanelWidthPx };
		event.preventDefault();
	}

	moveTagPanelResize(event: PointerEvent): void {
		if (!this.resizing || this.resizing.pointerId !== event.pointerId) return;
		// The handle sits on the panel's left edge, so dragging left widens it.
		this.tagPanelWidthPx = clampTagPanelWidth(this.resizing.startWidth + this.resizing.startX - event.clientX);
	}

	endTagPanelResize(event: PointerEvent): void {
		const handle = event.currentTarget as HTMLElement;
		if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
		if (this.resizing?.pointerId === event.pointerId) this.resizing = null;
	}

	cancelTagPanelResize(): void {
		this.resizing = null;
	}

	#drawerElement(drawer: CompactDrawer): HTMLElement | null {
		return drawer === "explorer" ? this.explorerDrawer : this.tagsDrawer;
	}
}
