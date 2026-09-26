/// <reference lib="webworker" />

import type { RawFrame } from "../../rawFrame";
import { renderRawFrameToRgba } from "../rawWindowing";
import type {
	WlRendererFailure,
	WlRendererRequest,
	WlRendererSuccess,
} from "./wlRendererProtocol";

let loaded: { frameId: number; frame: RawFrame } | null = null;

self.onmessage = async (event: MessageEvent<WlRendererRequest>) => {
	const payload = event.data;
	if (payload?.type === "frame") {
		loaded = {
			frameId: payload.frameId,
			frame: { metadata: payload.metadata, buffer: payload.buffer },
		};
		return;
	}
	if (payload?.type !== "render") {
		return;
	}

	try {
		if (!loaded || loaded.frameId !== payload.frameId) {
			throw new Error("render requested for a frame the worker does not hold");
		}
		const { metadata } = loaded.frame;
		const output = renderRawFrameToRgba(loaded.frame, payload.wc, payload.ww);
		const bitmap = await createImageBitmap(new ImageData(output, metadata.columns, metadata.rows));
		const response: WlRendererSuccess = {
			type: "rendered",
			id: payload.id,
			width: metadata.columns,
			height: metadata.rows,
			bitmap,
		};
		self.postMessage(response, [bitmap as unknown as Transferable]);
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		const response: WlRendererFailure = { type: "error", id: payload.id, message };
		self.postMessage(response);
	}
};
