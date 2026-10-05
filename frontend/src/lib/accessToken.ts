/**
 * The session's access token on the viewer's side. The launch link carries it
 * in the fragment (`#token=<t>`), which a browser never sends to a server;
 * `api.ts` sends it as a bearer header. It never goes into a URL the viewer
 * builds, and nothing here logs it.
 */

const FRAGMENT_KEY = "token";
const STORAGE_KEY = "dcmview.accessToken";
/** What the server issues or accepts: RFC 3986 unreserved characters, safe in a header. */
const TOKEN_PATTERN = /^[A-Za-z0-9\-._~]+$/;

/** The source of truth for this page; storage only carries it across a reload. */
let token: string | null = null;
let adopted = false;

/** Splits the fragment into its token (if any) and everything else, untouched. */
function splitFragment(hash: string): { found: boolean; value: string | null; rest: string } {
	const kept: string[] = [];
	let found = false;
	let value: string | null = null;
	for (const part of hash.replace(/^#/, "").split("&")) {
		if (part.startsWith(`${FRAGMENT_KEY}=`)) {
			found = true;
			const candidate = part.slice(FRAGMENT_KEY.length + 1);
			if (TOKEN_PATTERN.test(candidate)) value = candidate;
		} else if (part.length > 0) {
			kept.push(part);
		}
	}
	return { found, value, rest: kept.join("&") };
}

/** `sessionStorage` throws in some cross-site frames and private windows. */
function readStored(win: Window): string | null {
	try {
		const stored = win.sessionStorage.getItem(STORAGE_KEY);
		return stored !== null && TOKEN_PATTERN.test(stored) ? stored : null;
	} catch {
		return null;
	}
}

function store(win: Window, value: string): void {
	try {
		win.sessionStorage.setItem(STORAGE_KEY, value);
	} catch { /* The page still works from memory until it reloads. */ }
}

/**
 * Takes the token out of the address bar and keeps it for this tab. A token
 * in the fragment replaces any earlier one (a restarted server's new link
 * opened in the same tab); without one, a reload reads back what was stored.
 * The query string and any other fragment content are left as they were.
 * Returns whether the fragment brought a token this call.
 */
export function adoptAccessToken(win: Window = window): boolean {
	adopted = true;
	const fragment = splitFragment(win.location.hash);
	if (fragment.found) {
		const url = win.location.pathname + win.location.search + (fragment.rest ? `#${fragment.rest}` : "");
		try {
			win.history.replaceState(win.history.state, "", url);
		} catch { /* Nothing else to do: the token is still only on this machine. */ }
	}
	if (fragment.value !== null) {
		token = fragment.value;
		store(win, fragment.value);
		return true;
	}
	token ??= readStored(win);
	return false;
}

/** The token to send, or null when the page has none (a `--no-token` server needs none). */
export function accessToken(): string | null {
	if (!adopted && typeof window !== "undefined") adoptAccessToken();
	return token;
}
