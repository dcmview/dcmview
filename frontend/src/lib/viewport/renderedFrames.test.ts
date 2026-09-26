import { describe, expect, it } from "vitest";
import { RenderedFrames } from "./renderedFrames.svelte";

describe("RenderedFrames", () => {
	it("resolves a waiter when its frame is presented", async () => {
		const rendered = new RenderedFrames();
		const wait = rendered.waitFor(2, 5, new AbortController().signal);

		rendered.mark(2, 4);
		rendered.mark(2, 5);

		await expect(wait).resolves.toBe(true);
		expect(rendered.token).toBe("2:5");
		await expect(rendered.waitFor(2, 5, new AbortController().signal)).resolves.toBe(true);
	});

	it("keeps the presented frame when only the token is cleared", async () => {
		const rendered = new RenderedFrames();
		rendered.mark(1, 0);
		rendered.clearToken();

		expect(rendered.token).toBe("");
		await expect(rendered.waitFor(1, 0, new AbortController().signal)).resolves.toBe(true);
	});

	it("releases waiters unrendered on reset or abort", async () => {
		const rendered = new RenderedFrames();
		const ctrl = new AbortController();
		const aborted = rendered.waitFor(1, 1, ctrl.signal);
		const reset = rendered.waitFor(1, 2, new AbortController().signal);

		ctrl.abort();
		rendered.mark(1, 0);
		rendered.reset();

		await expect(aborted).resolves.toBe(false);
		await expect(reset).resolves.toBe(false);
		expect(rendered.token).toBe("");
	});
});
