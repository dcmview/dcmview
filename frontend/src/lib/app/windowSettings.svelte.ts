import type { WindowMode, WindowPreset } from "../../api";
import { WL_PRESETS } from "../viewerTools";

/** A manual window expressed relative to a file's own default window. */
type ManualWindowAdjustment = {
	centerOffsetRatio: number;
	widthRatio: number;
};

function usableWindow(window: WindowPreset | null | undefined): WindowPreset | null {
	if (!window || !Number.isFinite(window.center) || !Number.isFinite(window.width) || window.width <= 0) {
		return null;
	}
	return window;
}

/**
 * The viewer's window/level: an explicit center/width (from a preset or a
 * drag), a window mode, and the selected preset. A manual drag is also kept
 * relative to the file's default window, so moving to another file (such
 * as the next slice of a stack) carries the same adjustment over.
 *
 * A drag on a file with a real-world value mapping is recorded in that
 * mapping's `unit` (such as Gy): the viewport converts it to each frame's
 * stored scale, so it already means the same values on every frame and
 * file and is carried over unchanged.
 */
export class WindowSettings {
	center = $state<number | null>(null);
	width = $state<number | null>(null);
	/** Real-world unit of `center`/`width`; null for the rendered (Modality) scale. */
	unit = $state<string | null>(null);
	mode = $state<WindowMode>("default");
	presetId = $state("default");
	#manual: ManualWindowAdjustment | null = null;
	readonly #defaultWindow: (fileIndex: number) => WindowPreset | null | undefined;

	constructor(defaultWindow: (fileIndex: number) => WindowPreset | null | undefined) {
		this.#defaultWindow = defaultWindow;
	}

	/** Selects a toolbar preset: its fixed window, or the default/full-dynamic mode. */
	selectPreset(presetId: string): void {
		this.presetId = presetId;
		this.#manual = null;
		const preset = WL_PRESETS.find((candidate) => candidate.id === presetId);
		if (!preset) return;
		this.unit = null;
		if (preset.wc !== undefined && preset.ww !== undefined) {
			this.center = preset.wc;
			this.width = preset.ww;
			this.mode = "default";
		} else {
			this.center = null;
			this.width = null;
			this.mode = preset.mode ?? "default";
		}
	}

	/** Records a window/level drag on `fileIndex`, in real-world `unit` when given. */
	recordManual(fileIndex: number | null, center: number, width: number, unit: string | null = null): void {
		this.center = center;
		this.width = width;
		this.unit = unit;
		this.mode = "default";
		this.presetId = "default";
		this.#manual = null;
		if (unit !== null) return;
		if (fileIndex === null || !Number.isFinite(center) || !Number.isFinite(width) || width <= 0) return;
		const base = usableWindow(this.#defaultWindow(fileIndex));
		if (!base) {
			this.#manual = null;
			return;
		}
		this.#manual = {
			centerOffsetRatio: (center - base.center) / base.width,
			widthRatio: width / base.width,
		};
	}

	/** Re-applies a manual adjustment to a newly active file's default window. */
	followFile(fileIndex: number | null): void {
		if (fileIndex === null || !this.#manual) return;
		const base = usableWindow(this.#defaultWindow(fileIndex));
		if (!base) return;
		this.center = base.center + this.#manual.centerOffsetRatio * base.width;
		// Only the frame knows whether its values are integers: there LINEAR
		// applies, and the HUD reports, at least one unit (resolveDisplayWindow
		// and the server); continuous values keep a sub-unit width.
		this.width = Math.max(Number.MIN_VALUE, this.#manual.widthRatio * base.width);
		this.mode = "default";
	}

	/** Back to each file's default window. */
	reset(): void {
		this.#manual = null;
		this.center = null;
		this.width = null;
		this.unit = null;
		this.mode = "default";
		this.presetId = "default";
	}
}
