import { describe, expect, it, vi } from "vitest";

/** A tab: its address, its history and its session storage (which outlives a reload). */
function tab(address: string, storage: Storage = memoryStorage()) {
	const url = new URL(address);
	const location = { pathname: url.pathname, search: url.search, hash: url.hash };
	const replaceState = vi.fn((_state: unknown, _title: string, next: string) => {
		const replaced = new URL(next, url);
		Object.assign(location, { pathname: replaced.pathname, search: replaced.search, hash: replaced.hash });
	});
	const win = { location, history: { state: null, replaceState }, sessionStorage: storage } as unknown as Window;
	return { win, replaceState, shown: () => location.pathname + location.search + location.hash };
}

function memoryStorage(): Storage {
	const values = new Map<string, string>();
	return {
		getItem: (key: string) => values.get(key) ?? null,
		setItem: (key: string, value: string) => void values.set(key, value),
	} as unknown as Storage;
}

function refusingStorage(): Storage {
	const refuse = () => { throw new DOMException("denied", "SecurityError"); };
	return { getItem: refuse, setItem: refuse } as unknown as Storage;
}

/** A fresh page load: the module's in-memory token starts empty. */
async function loadPage() {
	vi.resetModules();
	return import("./accessToken");
}

describe("access token", () => {
	it.each([
		{ address: "http://127.0.0.1:8888/#token=abc-_.~09", token: "abc-_.~09", shown: "/" },
		{ address: "http://127.0.0.1:8888/?theme=dark#token=abc", token: "abc", shown: "/?theme=dark" },
		{ address: "http://host/viewer/?theme=dark&x=%20#frame=3&token=abc&file=a%2Fb", token: "abc", shown: "/viewer/?theme=dark&x=%20#frame=3&file=a%2Fb" },
		{ address: "http://127.0.0.1:8888/#token=", token: null, shown: "/" },
		{ address: "http://127.0.0.1:8888/#token=a%0Ab", token: null, shown: "/" },
		{ address: "http://127.0.0.1:8888/?token=query#section", token: null, shown: "/?token=query#section" },
	])("reads $address and leaves $shown in the address bar", async ({ address, token, shown }) => {
		const { adoptAccessToken, accessToken } = await loadPage();
		const page = tab(address);

		expect(adoptAccessToken(page.win)).toBe(token !== null);

		expect(accessToken()).toBe(token);
		expect(page.shown()).toBe(shown);
	});

	it("survives a reload of the tab, and a new link replaces the earlier token", async () => {
		const storage = memoryStorage();
		(await loadPage()).adoptAccessToken(tab("http://127.0.0.1:8888/#token=first", storage).win);

		const reloaded = await loadPage();
		const page = tab("http://127.0.0.1:8888/", storage);
		expect(reloaded.adoptAccessToken(page.win)).toBe(false);
		expect(reloaded.accessToken()).toBe("first");
		expect(page.replaceState).not.toHaveBeenCalled();

		// The server restarted and its new link was opened in the same tab.
		const relaunched = await loadPage();
		relaunched.adoptAccessToken(tab("http://127.0.0.1:8888/#token=second", storage).win);
		expect(relaunched.accessToken()).toBe("second");
		const again = await loadPage();
		again.adoptAccessToken(tab("http://127.0.0.1:8888/", storage).win);
		expect(again.accessToken()).toBe("second");
	});

	it("works from memory when session storage is refused", async () => {
		const { adoptAccessToken, accessToken } = await loadPage();
		const page = tab("http://127.0.0.1:8888/?theme=dark#token=abc", refusingStorage());

		expect(adoptAccessToken(page.win)).toBe(true);
		expect(accessToken()).toBe("abc");
		expect(page.shown()).toBe("/?theme=dark");

		// A page that never had a token, in the same kind of frame.
		const bare = await loadPage();
		expect(bare.adoptAccessToken(tab("http://127.0.0.1:8888/", refusingStorage()).win)).toBe(false);
		expect(bare.accessToken()).toBeNull();
	});
});
