import type { FrameRefSummary, SeriesStackSummary, SeriesSummary } from "../api";

export type NavigationFrameRef = Pick<
	FrameRefSummary,
	"virtual_index" | "file_index" | "frame_index"
>;

export interface LocatedSeriesStack {
	series: SeriesSummary;
	stack: SeriesStackSummary;
}

export function findSeriesStackForFile(
	series: readonly SeriesSummary[],
	fileIndex: number,
): LocatedSeriesStack | null {
	for (const item of series) {
		for (const stack of item.stacks) {
			if (stack.frames.some((frame) => frame.file_index === fileIndex)) {
				return { series: item, stack };
			}
		}
	}
	return null;
}

export function framePosition(
	stack: SeriesStackSummary,
	fileIndex: number,
	frameIndex: number,
): number | null {
	const exact = stack.frames.findIndex(
		(frame) => frame.file_index === fileIndex && frame.frame_index === frameIndex,
	);
	if (exact >= 0) return exact;
	const source = stack.frames.findIndex((frame) => frame.file_index === fileIndex);
	return source >= 0 ? source : null;
}

export function frameAtPosition(
	stack: SeriesStackSummary | null,
	position: number,
): FrameRefSummary | null {
	if (!stack || stack.frames.length === 0) return null;
	const bounded = Math.max(0, Math.min(stack.frames.length - 1, position));
	return stack.frames[bounded] ?? null;
}

export function navigationTabId(
	series: readonly SeriesSummary[],
	fileIndex: number,
): string {
	return findSeriesStackForFile(series, fileIndex)?.stack.id ?? `file:${fileIndex}`;
}

export function navigationFramesForFile(
	fileIndex: number,
	frameCount: number,
): NavigationFrameRef[] {
	return Array.from({ length: Math.max(0, frameCount) }, (_, frameIndex) => ({
		virtual_index: frameIndex,
		file_index: fileIndex,
		frame_index: frameIndex,
	}));
}

export function navigationFrameAtPosition(
	frames: readonly NavigationFrameRef[],
	position: number,
): NavigationFrameRef | null {
	if (frames.length === 0) return null;
	const bounded = Math.max(0, Math.min(frames.length - 1, position));
	return frames[bounded] ?? null;
}

/** `file:frame` of every frame within `distance` positions of `position`. */
export function framesNear(
	frames: readonly NavigationFrameRef[],
	position: number,
	distance: number,
): Set<string> {
	const near = new Set<string>();
	const last = Math.min(frames.length - 1, position + distance);
	for (let index = Math.max(0, position - distance); index <= last; index += 1) {
		near.add(`${frames[index].file_index}:${frames[index].frame_index}`);
	}
	return near;
}
