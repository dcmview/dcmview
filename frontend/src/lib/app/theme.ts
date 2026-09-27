/**
 * The viewer follows the OS theme (prefers-color-scheme) unless its host asks
 * for one. The VS Code extension passes `?theme=` in the viewer URL and posts
 * `{ type: "dcmview-theme", theme }` when the editor theme changes
 * (vscode/src/viewerSessions.ts).
 */
export type Theme = "light" | "dark";

export const THEME_MESSAGE_TYPE = "dcmview-theme";

export function parseTheme(value: unknown): Theme | undefined {
	return value === "light" || value === "dark" ? value : undefined;
}

/** Sets data-theme on the root with transitions held for one frame, so the switch is instant. */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
	root.classList.add("theme-switching");
	root.dataset.theme = theme;
	requestAnimationFrame(() => requestAnimationFrame(() => root.classList.remove("theme-switching")));
}

/** Applies the theme requested in the URL, then follows theme messages from the parent frame. */
export function followHostTheme(win: Window = window): void {
	const requested = parseTheme(new URLSearchParams(win.location.search).get("theme"));
	if (requested) applyTheme(requested, win.document.documentElement);
	if (win.parent === win) return;
	win.addEventListener("message", (event) => {
		if (event.source !== win.parent || event.data?.type !== THEME_MESSAGE_TYPE) return;
		const theme = parseTheme(event.data.theme);
		if (theme) applyTheme(theme, win.document.documentElement);
	});
}
