import type { DisplayFrame, FrameValueMapping, RawFrame, WindowMode, WindowPreset } from "../../api";
import { resolveDisplayWindow, resolveMappedDisplayWindow, samplePresentation, type ResolvedWindow } from "../rawWindowing";
import { mappedWindowScale, windowToMapped, windowToRender } from "./valueMapping";

export type WindowChoice = { window: ResolvedWindow; unit: string | null };
export type WindowSource = "live" | "explicit" | "automatic" | "full_dynamic" | "dicom" | "server" | "real_world" | "fallback" | "voi_lut" | "color" | "pending";
export type WindowResolution = WindowChoice & { source: WindowSource } | { window: null; unit: null; source: WindowSource };
export type WindowInput = {
	raw: RawFrame | null;
	mapping: FrameValueMapping | null;
	requested: WindowChoice | null;
	live: WindowChoice | null;
	mode: WindowMode;
	defaultWindow: WindowPreset | null;
	server: Pick<DisplayFrame, "window" | "appliedWindow"> | null;
	/** The actual HTTP request carried unit, rather than a converted Modality window. */
	unitRequest: boolean;
};

/** One decision for the displayed window, its unit, and why it applies. */
export function resolveWindow(input: WindowInput): WindowResolution {
	const { raw, mapping, requested, live, mode, server } = input;
	const map = mapping?.real_world[0];
	const scale = mappedWindowScale(mapping);
	const mapped = (window: ResolvedWindow | null, source: WindowSource): WindowResolution => {
		if (!window) return { window: null, unit: null, source };
		if (window.voiLut) return { window, unit: null, source: "voi_lut" };
		if (!scale) return { window, unit: null, source };
		const value = windowToMapped({ center: window.wc, width: window.ww }, scale);
		return { window: { wc: value.center, ww: value.width }, unit: scale.unit, source };
	};
	// Color has no scalar window, even if the toolbar has a preset selected.
	if (raw?.metadata.samplesPerPixel !== undefined && raw.metadata.samplesPerPixel !== 1
		|| !raw && server?.appliedWindow === null && server.window === null) {
		return { window: null, unit: null, source: "color" };
	}
	// A drag is explicit user intent and immediately leaves automatic/VOI presentation.
	if (live) return { ...live, source: "live" };
	if (raw) {
		if (!mapping) return { window: null, unit: null, source: "pending" };
		const applicable = requested?.unit == null || requested.unit === map?.unit_label;
		const request = applicable ? requested : null;
		const source = mode === "full_dynamic" ? "full_dynamic" : request ? "explicit"
			: raw.metadata.defaultWc !== null ? "dicom" : "automatic";
		if (map && !scale && (!request || request.unit === map.unit_label)) {
			const window = resolveMappedDisplayWindow(raw, map, null, null,
				request?.window.wc ?? null, request?.window.ww ?? null, mode);
			return { window, unit: map.unit_label, source };
		}
		const converted = request?.unit && scale
			? windowToRender({ center: request.window.wc, width: request.window.ww }, scale) : null;
		const window = resolveDisplayWindow(raw, null, null,
			converted?.center ?? request?.window.wc ?? null,
			converted?.width ?? request?.window.ww ?? null, mode, samplePresentation(raw, mapping));
		return mapped(window, source);
	}
	if (server) {
		if (server.appliedWindow === "voi_lut") return { window: null, unit: null, source: "voi_lut" };
		if (input.unitRequest) {
			if (server.appliedWindow === "real_world" && requested?.unit) return { ...requested, source: "real_world" };
			return { window: server.window, unit: null, source: "fallback" };
		}
		return mapped(server.window, "server");
	}
	const fallback = input.defaultWindow;
	return mapped(fallback ? { wc: fallback.center, ww: fallback.width } : null, "pending");
}
