// @vitest-environment happy-dom
import { fireEvent, render, screen } from "@testing-library/svelte";
import { expect, it, vi } from "vitest";
import * as api from "../api";
import TagPanel from "./TagPanel.svelte";
vi.mock("../api", async (original) => ({ ...await original<typeof import("../api")>(), fetchTags: vi.fn() }));

it("leaves child button activation native and keeps copy/expand keys out of global shortcuts", async () => {
	const copy = vi.fn(async () => {});
	Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: copy } });
	vi.mocked(api.fetchTags).mockResolvedValue([
		{ tag: "(0008,1115)", keyword: "ReferencedSeriesSequence", vr: "SQ", value: { type: "sequence", items: [[]] } },
		{ tag: "(0020,4000)", keyword: "ImageComments", vr: "LT", value: { type: "string", value: "x".repeat(257) } },
	]);
	render(TagPanel, { props: { fileIndex: 1 } });
	const expand = await screen.findByRole("button", { name: "Expand sequence" });
	const globalKey = vi.fn(); window.addEventListener("keydown", globalKey);
	try {
		for (const key of ["Enter", " "]) {
			const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
			expand.dispatchEvent(event);
			expect(event.defaultPrevented).toBe(false);
		}
		expect(globalKey).not.toHaveBeenCalled();
		expect(copy).not.toHaveBeenCalled();
		await fireEvent.click(expand);
		expect(expand.getAttribute("aria-expanded")).toBe("true");
		const value = screen.getByRole("button", { name: "x".repeat(80) + "…" });
		await fireEvent.keyDown(value, { key: " " });
		await fireEvent.click(value);
		expect(value.textContent?.trim()).toBe("x".repeat(257));
		const rowCopy = screen.getByRole("button", { name: "Copy (0020,4000) ImageComments" });
		await fireEvent.keyDown(rowCopy, { key: " " });
		await fireEvent.click(rowCopy);
		expect(copy).toHaveBeenCalledWith(expect.stringContaining("x".repeat(257)));
		expect(globalKey).not.toHaveBeenCalled();
		expect(expand.parentElement?.closest('button, [role="button"]')).toBeNull();
		expect(value.parentElement?.closest('button, [role="button"]')).toBeNull();
	} finally { window.removeEventListener("keydown", globalKey); }
});
