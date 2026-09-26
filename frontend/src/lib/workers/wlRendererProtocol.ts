import type { RealWorldValueMap } from "../../generated/api-types";
import type { RawFrameMetadata } from "../../rawFrame";

/** Hands the worker the frame that later render requests window. */
export type WlRendererLoadFrame = {
	type: "frame";
	frameId: number;
	metadata: RawFrameMetadata;
	buffer: ArrayBuffer;
};

/** Renders the loaded frame; only the window travels per request. */
export type WlRendererRender = {
	type: "render";
	id: number;
	frameId: number;
	wc: number;
	ww: number;
	/** Window this real-world mapping's values instead of Modality values. */
	valueMap: RealWorldValueMap | null;
};

export type WlRendererRequest = WlRendererLoadFrame | WlRendererRender;

export type WlRendererSuccess = {
	type: "rendered";
	id: number;
	width: number;
	height: number;
	bitmap: ImageBitmap;
};

export type WlRendererFailure = {
	type: "error";
	id: number;
	message: string;
};

export type WlRendererResponse = WlRendererSuccess | WlRendererFailure;
