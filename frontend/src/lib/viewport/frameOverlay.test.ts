import { describe, expect, it, vi } from "vitest";
import type { FileSummary } from "../../api";
import {
	composeOverlayFrame,
	overlayLayerRequests,
	type DecodedCanvasImage,
	type SegmentationOverlay,
} from "./frameOverlay";

function image(name: string, width: number, height: number): DecodedCanvasImage {
	return { source: { name } as unknown as CanvasImageSource, width, height, dispose: vi.fn() };
}

describe("frame overlays", () => {
	it("requests one mask layer per SEG frame, keyed for sharing", () => {
		const overlay: SegmentationOverlay = {
			kind: "segmentation",
			segmentationFileIndex: 4,
			segmentationFrameIndex: 2,
			sourceFileIndex: 1,
			sourceFrameIndex: 0,
			sourceFile: {} as FileSummary,
		};
		expect(overlayLayerRequests(overlay).map((layer) => layer.key)).toEqual(["seg:4:2"]);
	});

	it("draws the base at its size and stretches each layer over it in order", () => {
		const drawImage = vi.fn();
		const canvas = { width: 0, height: 0, getContext: () => ({ drawImage }) } as unknown as HTMLCanvasElement;
		const base = image("base", 64, 32);
		const mask = image("mask", 16, 8);

		composeOverlayFrame(canvas, base, [mask]);

		expect([canvas.width, canvas.height]).toEqual([64, 32]);
		expect(drawImage.mock.calls).toEqual([
			[base.source, 0, 0],
			[mask.source, 0, 0, 64, 32],
		]);
	});

	it("fails loudly when the canvas has no 2D context", () => {
		const canvas = { width: 0, height: 0, getContext: () => null } as unknown as HTMLCanvasElement;
		expect(() => composeOverlayFrame(canvas, image("base", 1, 1), [])).toThrow("2D canvas is unavailable");
	});
});
