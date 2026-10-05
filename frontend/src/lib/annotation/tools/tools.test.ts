import { describe, expect, it } from "vitest";
import type { EmbedRoiAnnotations } from "../../../api";
import { fileSummary } from "../../../testing/fixtures";
import { visibleRois } from "../../viewport/roiEditing";
import { zoomAnchor, zoomAroundAnchor, type ViewTransform } from "../../viewport/viewTransform";
import { PanTool } from "./panTool";
import { RectangleTool } from "./rectangleTool";
import { ScrollTool } from "./scrollTool";
import type { Tool, ToolContext } from "./tool";
import { WindowLevelTool } from "./windowLevelTool";
import { ZoomTool } from "./zoomTool";

type Step =
	| ["down", number, number]
	| ["move", number, number]
	| ["wheel", number]
	| ["up"]
	| ["cancel"];

type Viewport = {
	/** Images in the stack, and the one shown. */
	images?: number;
	position?: number;
	/** The file's rectangles; null while they load. */
	rectangles?: EmbedRoiAnnotations | null;
	redacting?: boolean;
	/** False for a frame that cannot be windowed now. */
	windowable?: boolean;
};

const oneRectangle: EmbedRoiAnnotations = { num_roi: 1, roi_coords: [[10, 10, 30, 30]], roi_frames: [[1]] };
const noRectangles: EmbedRoiAnnotations = { num_roi: 0, roi_coords: [], roi_frames: [] };

/**
 * A 64x64, three-frame file shown on frame 1 at 1:1 with the image at the
 * client origin, so client and image coordinates agree. `effects` lists what
 * the tool did to the viewport, in order.
 */
function viewport({ images = 1, position = 0, rectangles = noRectangles, redacting = false, windowable = true }: Viewport) {
	const effects: unknown[][] = [];
	const origin = { left: 0, top: 0 };
	const file = fileSummary(5, { frame_count: 3, rows: 64, columns: 64 });
	let transform: ViewTransform = { scale: 1, tx: 0, ty: 0, fit: false };
	let shown = rectangles;
	let selected: number | null = null;
	const ctx: ToolContext = {
		file,
		frame: 1,
		imageRows: 64,
		imageColumns: 64,
		get transform() { return transform; },
		setTransform(next) {
			transform = { ...next, fit: false };
			effects.push(["transform", Number(next.scale.toFixed(3)), Math.round(next.tx), Math.round(next.ty)]);
		},
		toImage: (clientX, clientY) => ({ x: Math.min(64, Math.max(0, clientX)), y: Math.min(64, Math.max(0, clientY)) }),
		zoomAnchor: (clientX, clientY) => zoomAnchor(clientX, clientY, origin, transform),
		zoomTransform: (scale, anchor) => zoomAroundAnchor(scale, anchor, origin),
		navigation: {
			count: images,
			position,
			go: (next) => effects.push(["go", next]),
		},
		window: {
			begin: () => (windowable ? { center: 40, width: 400, step: 2 } : null),
			preview: (center, width) => effects.push(["preview", center, width]),
			commit: () => effects.push(["commit window"]),
		},
		rects: {
			get editable() { return shown !== null; },
			get visible() { return visibleRois(shown, 1); },
			get annotations() { return shown; },
			get selectedIndex() { return selected; },
			coversAllFrames: redacting,
			select: (index) => { selected = index; },
			beginLiveEdit: () => {},
			showDraft: (fileIndex, annotations) => {
				shown = annotations;
				effects.push(["show", fileIndex, ...annotations.roi_coords]);
			},
			commit: (annotations, selectedIndex) => {
				shown = annotations;
				effects.push(["save", annotations.roi_coords, annotations.roi_frames, selectedIndex]);
			},
		},
	};
	return { ctx, effects };
}

function run(tool: Tool, ctx: ToolContext, steps: Step[]): unknown[] {
	const answers: unknown[] = [];
	for (const step of steps) {
		if (step[0] === "down") answers.push(tool.pointerDown({ clientX: step[1], clientY: step[2] }, ctx));
		else if (step[0] === "move") tool.pointerMove({ clientX: step[1], clientY: step[2] }, ctx);
		else if (step[0] === "wheel") answers.push(tool.wheel?.({ dx: 0, dy: step[1] }, ctx) ?? false);
		else if (step[0] === "up") tool.pointerUp(ctx);
		else tool.cancel(ctx);
	}
	return answers;
}

const cases: { name: string; tool: () => Tool; viewport?: Viewport; steps: Step[]; answers: unknown[]; effects: unknown[][] }[] = [
	{
		name: "pan follows the pointer from where the drag began, and stops on release",
		tool: () => new PanTool(),
		steps: [["down", 10, 10], ["move", 25, 4], ["move", 12, 40], ["up"], ["move", 90, 90]],
		answers: ["capture"],
		effects: [["transform", 1, 15, -6], ["transform", 1, 2, 30]],
	},
	{
		name: "zoom doubles about the pressed point for ln(2)/0.005 px of upward drag",
		tool: () => new ZoomTool(),
		steps: [["down", 20, 100], ["move", 500, 100 - Math.log(2) / 0.005], ["move", 20, 100 + Math.log(2) / 0.005]],
		answers: ["capture"],
		effects: [["transform", 2, -20, -100], ["transform", 0.5, 10, 50]],
	},
	{
		name: "scroll drag steps one image per 10/0.7 px and stops at both ends",
		tool: () => new ScrollTool(),
		viewport: { images: 10, position: 4 },
		steps: [["down", 0, 100], ["move", 0, 107], ["move", 0, 108], ["move", 0, 129], ["move", 0, 900], ["move", 0, -900], ["up"], ["move", 0, 300]],
		answers: ["capture"],
		effects: [["go", 4], ["go", 5], ["go", 6], ["go", 9], ["go", 0]],
	},
	{
		name: "scroll wheel steps one image per event in the wheel's direction",
		tool: () => new ScrollTool(),
		viewport: { images: 10, position: 4 },
		steps: [["wheel", 120], ["wheel", -3], ["wheel", 0]],
		answers: [true, true, false],
		effects: [["go", 5], ["go", 3]],
	},
	{
		name: "scroll leaves a single image's drags and wheel to the viewport",
		tool: () => new ScrollTool(),
		steps: [["down", 0, 100], ["move", 0, 200], ["wheel", 120]],
		answers: ["ignore", false],
		effects: [],
	},
	{
		name: "window/level widens rightward by four steps per px, lowers the center downward by two, and reports on release",
		tool: () => new WindowLevelTool(),
		steps: [["down", 10, 10], ["move", 20, 15], ["move", -1000, 10], ["up"], ["move", 30, 30]],
		answers: ["capture"],
		effects: [["preview", 20, 480], ["preview", 40, 2], ["commit window"]],
	},
	{
		name: "window/level ignores a frame that cannot be windowed",
		tool: () => new WindowLevelTool(),
		viewport: { windowable: false },
		steps: [["down", 10, 10], ["move", 20, 15], ["up"]],
		answers: ["ignore"],
		effects: [],
	},
	{
		name: "ROI drag on empty image saves a rectangle on the shown frame, selected",
		tool: () => new RectangleTool("annotate_rect"),
		steps: [["down", 40.4, 50], ["move", 12, 5.6], ["up"]],
		answers: ["capture"],
		effects: [["save", [[6, 12, 50, 40]], [[1]], 0]],
	},
	{
		name: "redaction drag saves a box on every frame and keeps it inside the image",
		tool: () => new RectangleTool("redact"),
		viewport: { redacting: true },
		steps: [["down", 40, 50], ["move", 500, 500], ["up"]],
		answers: ["capture"],
		effects: [["save", [[50, 40, 64, 64]], [[0, 1, 2]], 0]],
	},
	{
		name: "a press without a drag, or a sliver under two pixels, saves nothing",
		tool: () => new RectangleTool("annotate_rect"),
		steps: [["down", 40, 50], ["up"], ["down", 40, 50], ["move", 60, 51], ["up"]],
		answers: ["capture", "capture"],
		effects: [],
	},
	{
		name: "dragging inside a rectangle moves it within the image and saves once on release",
		tool: () => new RectangleTool("annotate_rect"),
		viewport: { rectangles: oneRectangle },
		steps: [["down", 20, 20], ["move", 25, 22], ["move", 500, 20], ["up"]],
		answers: ["capture"],
		effects: [["show", 5, [12, 15, 32, 35]], ["show", 5, [10, 44, 30, 64]], ["save", [[10, 44, 30, 64]], [[1]], 0]],
	},
	{
		name: "dragging a corner handle resizes the rectangle from its original corner",
		tool: () => new RectangleTool("annotate_rect"),
		viewport: { rectangles: oneRectangle },
		steps: [["down", 31, 29], ["move", 50, 45], ["up"]],
		answers: ["capture"],
		effects: [["show", 5, [10, 10, 45, 50]], ["save", [[10, 10, 45, 50]], [[1]], 0]],
	},
	{
		name: "a cancelled move puts the rectangle back without saving",
		tool: () => new RectangleTool("annotate_rect"),
		viewport: { rectangles: oneRectangle },
		steps: [["down", 20, 20], ["move", 25, 22], ["cancel"], ["move", 40, 40], ["up"]],
		answers: ["capture"],
		effects: [["show", 5, [12, 15, 32, 35]], ["show", 5, [10, 10, 30, 30]]],
	},
	{
		name: "rectangles cannot be drawn before the file's own have loaded",
		tool: () => new RectangleTool("annotate_rect"),
		viewport: { rectangles: null },
		steps: [["down", 40, 50], ["move", 12, 5], ["up"]],
		answers: ["ignore"],
		effects: [],
	},
];

describe("viewport tools", () => {
	it.each(cases)("$name", ({ tool, viewport: shown = {}, steps, answers, effects }) => {
		const { ctx, effects: seen } = viewport(shown);
		expect(run(tool(), ctx, steps)).toEqual(answers);
		expect(seen).toEqual(effects);
	});
});
