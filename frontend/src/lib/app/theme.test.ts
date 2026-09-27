// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { followHostTheme, THEME_MESSAGE_TYPE } from "./theme";

function hostWindow(search: string) {
	const listeners: ((event: MessageEvent) => void)[] = [];
	const parent = {} as Window;
	const root = document.createElement("html");
	const win = {
		location: { search },
		document: { documentElement: root },
		parent,
		addEventListener: (_type: string, listener: (event: MessageEvent) => void) => listeners.push(listener),
	} as unknown as Window;
	const post = (data: unknown, source: unknown = parent) =>
		listeners.forEach((listener) => listener({ data, source } as MessageEvent));
	return { win, root, parent, post };
}

describe("followHostTheme", () => {
	it("applies a valid theme from the URL and ignores anything else", () => {
		const dark = hostWindow("?theme=dark");
		followHostTheme(dark.win);
		expect(dark.root.dataset.theme).toBe("dark");

		const bogus = hostWindow("?theme=sepia");
		followHostTheme(bogus.win);
		expect(bogus.root.dataset.theme).toBeUndefined();
	});

	it("follows theme messages from the parent frame only", () => {
		const host = hostWindow("?theme=dark");
		followHostTheme(host.win);

		host.post({ type: THEME_MESSAGE_TYPE, theme: "light" }, {});
		expect(host.root.dataset.theme).toBe("dark");

		host.post({ type: THEME_MESSAGE_TYPE, theme: "light" });
		expect(host.root.dataset.theme).toBe("light");

		host.post({ type: THEME_MESSAGE_TYPE, theme: "neon" });
		expect(host.root.dataset.theme).toBe("light");
	});
});
