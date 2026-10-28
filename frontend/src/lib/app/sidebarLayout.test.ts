import { expect, it } from "vitest";
import {
	clampSidebarWidth,
	FILE_NAV_MAX_WIDTH_PX,
	FILE_NAV_MIN_WIDTH_PX,
	TAG_PANEL_MAX_WIDTH_PX,
	TAG_PANEL_MIN_WIDTH_PX,
} from "./sidebarLayout.svelte";

it("keeps each sidebar within its own range", () => {
	expect(clampSidebarWidth("explorer", 10)).toBe(FILE_NAV_MIN_WIDTH_PX);
	expect(clampSidebarWidth("explorer", 10_000)).toBe(FILE_NAV_MAX_WIDTH_PX);
	expect(clampSidebarWidth("tags", 10)).toBe(TAG_PANEL_MIN_WIDTH_PX);
	expect(clampSidebarWidth("tags", 10_000)).toBe(TAG_PANEL_MAX_WIDTH_PX);
	expect(clampSidebarWidth("explorer", 300)).toBe(300);
});

it("stops widening where the image would get narrower than 320px", () => {
	// 1200 - 420 (tag panel) - 320 leaves 460 for the explorer.
	expect(clampSidebarWidth("explorer", 540, 420, 1200)).toBe(460);
	// The sidebar's own minimum still wins in a very small window.
	expect(clampSidebarWidth("tags", 500, 276, 600)).toBe(TAG_PANEL_MIN_WIDTH_PX);
});
