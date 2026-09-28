import type { RawFrameMetadata } from "../../rawFrame";
import type { RenderOptions } from "../rawWindowing";

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
	options: RenderOptions;
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
