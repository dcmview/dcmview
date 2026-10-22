/**
 * Minification for the full-resolution frame canvases. The image layer is a
 * canvas holding every stored pixel, scaled by a CSS transform; shown below
 * one device pixel per image pixel, the browser samples it (nearest
 * neighbour under `image-rendering: pixelated`, plain bilinear otherwise)
 * and skips whole rows and columns, so one-pixel detail such as a thin line
 * or a calcification comes and goes as the zoom changes. Below that scale
 * the layer is drawn from a copy halved until it is within a factor of
 * √2 of its size on screen, each halving an average of 2x2 pixels, and
 * the browser scales that copy the rest of the way.
 */

/** How a canvas shown at a device scale is drawn. */
export type Minification = {
	/** Times the canvas is halved; 0 draws the canvas itself. */
	level: number;
	/** True below one device pixel per pixel, where the browser should smooth. */
	smooth: boolean;
};

/**
 * The minification for a canvas of `width` x `height` pixels shown at
 * `deviceScale` device pixels per canvas pixel along its more reduced axis.
 * Bilinear scaling misses a one-pixel line by up to its whole weight when it
 * reduces by nearly half, and by less than a third of it within √2 either
 * way, so the copy is the halving nearest the screen size, and at least one
 * pixel wide.
 */
export function minificationFor(deviceScale: number, width: number, height: number): Minification {
	if (!(deviceScale > 0) || deviceScale >= 1) return { level: 0, smooth: false };
	const wanted = Math.round(-Math.log2(deviceScale));
	const largest = Math.floor(Math.log2(Math.max(1, Math.min(width, height))));
	return { level: Math.min(wanted, largest), smooth: true };
}

/** The size of a canvas after `level` halvings, odd sizes rounding up. */
export function levelSize(width: number, height: number, level: number): { width: number; height: number } {
	let w = width;
	let h = height;
	for (let i = 0; i < level; i += 1) {
		w = Math.max(1, Math.ceil(w / 2));
		h = Math.max(1, Math.ceil(h / 2));
	}
	return { width: w, height: h };
}

type CreateCanvas = () => HTMLCanvasElement;

/**
 * Keeps `target` a reduced copy of a source canvas. `update` redraws it
 * after the source changes or the level does; only the copy shown is kept,
 * and the intermediate halvings are released once it is drawn.
 */
export class MinifiedCanvas {
	readonly #alpha: boolean;
	readonly #createCanvas: CreateCanvas;

	constructor({ alpha, createCanvas = () => document.createElement("canvas") }: {
		alpha: boolean;
		createCanvas?: CreateCanvas;
	}) {
		this.#alpha = alpha;
		this.#createCanvas = createCanvas;
	}

	/**
	 * Draws `source` halved `level` times into `target`, or empties `target`
	 * at level 0, where the source is shown itself.
	 */
	update(source: HTMLCanvasElement, target: HTMLCanvasElement, level: number): void {
		if (level <= 0 || source.width === 0 || source.height === 0) {
			target.width = 0;
			target.height = 0;
			return;
		}
		const scratch: HTMLCanvasElement[] = [];
		let from: HTMLCanvasElement = source;
		try {
			for (let step = 1; step <= level; step += 1) {
				const into = step === level ? target : (scratch[step % 2] ??= this.#createCanvas());
				const size = levelSize(from.width, from.height, 1);
				into.width = size.width;
				into.height = size.height;
				const ctx = into.getContext("2d", { alpha: this.#alpha });
				if (!ctx) return;
				// An exact halving samples bilinearly between each 2x2 block,
				// which is their average.
				ctx.imageSmoothingEnabled = true;
				ctx.imageSmoothingQuality = "low";
				if (this.#alpha) ctx.clearRect(0, 0, size.width, size.height);
				ctx.drawImage(from, 0, 0, size.width, size.height);
				from = into;
			}
		} finally {
			for (const canvas of scratch) {
				if (!canvas) continue;
				canvas.width = 0;
				canvas.height = 0;
			}
		}
	}
}
