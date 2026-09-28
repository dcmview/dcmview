import type { DisplayFrameWindowOptions } from "../../api";
import {
	runRenderPacedCine,
	waitForCineDeadline,
	type CineDirection,
	type CineMode,
} from "../cinePlayback";
import { navigationFrameAtPosition, type NavigationFrameRef } from "../seriesNavigation";
import type { DisplayFrameSource } from "./displayFrameSource";
import type { RenderedFrames } from "./renderedFrames.svelte";

export type DisplayCineOptions = {
	frames: readonly NavigationFrameRef[];
	startPosition: number;
	direction: CineDirection;
	mode: CineMode;
	fps: number;
	windowOptions: DisplayFrameWindowOptions;
	display: Pick<DisplayFrameSource, "ensureBlob" | "decode" | "key">;
	rendered: Pick<RenderedFrames, "waitFor">;
	signal: AbortSignal;
	/** Moves the view to `position`; playback resumes once that frame is presented. */
	onstep: (position: number, direction: CineDirection) => void;
};

/**
 * Plays display frames paced by actual presentation: each next frame is
 * fetched and decoded while the interval elapses, then shown, and playback
 * waits for the canvas to present it before scheduling the next one.
 * Resolves when `signal` aborts or a frame is never presented.
 */
export async function playDisplayCine({
	frames,
	startPosition,
	direction,
	mode,
	fps,
	windowOptions,
	display,
	rendered,
	signal,
	onstep,
}: DisplayCineOptions): Promise<void> {
	const initialFrame = navigationFrameAtPosition(frames, startPosition);
	if (!initialFrame) return;
	if (!await rendered.waitFor(initialFrame.file_index, initialFrame.frame_index, signal)) return;
	await runRenderPacedCine({
		initialFrame: startPosition,
		totalFrames: frames.length,
		mode,
		direction,
		fps,
		signal,
		now: () => performance.now(),
		waitForDelay: waitForCineDeadline,
		prepareFrame: (position) => {
			const frame = navigationFrameAtPosition(frames, position);
			if (!frame) return Promise.reject(new Error("logical cine frame is unavailable"));
			return display.ensureBlob(frame.file_index, frame.frame_index, windowOptions)
				.then((blob) => typeof createImageBitmap === "function"
					? display.decode(display.key(frame.file_index, frame.frame_index, windowOptions), blob)
						.then((decoded) => decoded.release())
					: undefined);
		},
		presentFrame: async (step, stepSignal) => {
			const frame = navigationFrameAtPosition(frames, step.frame);
			if (!frame) return false;
			onstep(step.frame, step.direction);
			return rendered.waitFor(frame.file_index, frame.frame_index, stepSignal);
		},
	});
}
