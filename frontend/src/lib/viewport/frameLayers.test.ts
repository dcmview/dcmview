import { describe, expect, it, vi } from "vitest";
import { ApiError, fetchDoseOverlayBlob, fetchPresentationLayerBlob } from "../../api";
import { fileSummary } from "../../testing/fixtures";
import { decodeCanvasImage, type ValueOverlay } from "./frameOverlay";
import { FrameLayers } from "./frameLayers";

vi.mock("../../api", async (original) => ({ ...await original<typeof import("../../api")>(),
	fetchDoseOverlayBlob: vi.fn(), fetchPresentationLayerBlob: vi.fn(),
}));
vi.mock("./frameOverlay", async (original) => ({ ...await original<typeof import("./frameOverlay")>(),
	decodeCanvasImage: vi.fn(async () => ({ source: {} as CanvasImageSource, width: 2, height: 2, dispose: vi.fn() })),
}));
const overlay = { kind: "rt_dose", volumeFileIndex: 9, coversFrame: true } as ValueOverlay;

describe("FrameLayers", () => {
	it("shares prefetched colorwash payloads and waits for both layers before presentation", async () => {
		const valueBlob = new Blob(["dose"]);
		let finish!: (blob: Blob) => void;
		vi.mocked(fetchDoseOverlayBlob).mockResolvedValue(valueBlob);
		vi.mocked(fetchPresentationLayerBlob).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
		const layers = new FrameLayers();
		await layers.value(overlay, 1, 0);
		const ready = vi.fn();
		const pending = layers.prepare(fileSummary(1, { presentation_layer: true }), 0, true, overlay).then(ready);
		await Promise.resolve();
		expect(ready).not.toHaveBeenCalled();
		finish(new Blob(["shutter"]));
		await pending;
		const prepared = ready.mock.calls[0][0];
		expect(prepared.presentation.status).toBe("shown");
		expect(prepared.value.status).toBe("shown");
		expect(fetchDoseOverlayBlob).toHaveBeenCalledTimes(1);
		prepared.dispose();
		expect(prepared.presentation.image.dispose).toHaveBeenCalledOnce();
		expect(prepared.value.image.dispose).toHaveBeenCalledOnce();
	});

	it("returns independent failure status so a missing layer does not discard the image", async () => {
		vi.mocked(fetchPresentationLayerBlob).mockRejectedValue(new Error("presentation unavailable"));
		vi.mocked(fetchDoseOverlayBlob).mockRejectedValue(new ApiError("outside", 404, "overlay_not_covering_frame"));
		const result = await new FrameLayers().prepare(fileSummary(1, { presentation_layer: true }), 0, true, overlay);
		expect(result.presentation.status).toBe("error");
		expect(result.value.status).toBe("not_covering");
	});

	it("does not decode or request layers absent from the frame's presentation", async () => {
		vi.mocked(fetchDoseOverlayBlob).mockClear(); vi.mocked(fetchPresentationLayerBlob).mockClear(); vi.mocked(decodeCanvasImage).mockClear();
		const result = await new FrameLayers().prepare(fileSummary(1, { presentation_layer: true }), 0, false, { ...overlay, coversFrame: false });
		expect(result.presentation.status).toBe("none");
		expect(result.value.status).toBe("not_covering");
		expect(fetchPresentationLayerBlob).not.toHaveBeenCalled();
		expect(fetchDoseOverlayBlob).not.toHaveBeenCalled();
		expect(decodeCanvasImage).not.toHaveBeenCalled();
	});
});
