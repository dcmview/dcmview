import { describe, expect, it } from "vitest";
import { levelSize, MinifiedCanvas, minificationFor } from "./minifiedCanvas";

type Draw = { from: FakeCanvas; width: number; height: number; into: FakeCanvas };

class FakeCanvas {
	width: number;
	height: number;
	constructor(width = 300, height = 150, readonly draws: Draw[] = []) {
		this.width = width;
		this.height = height;
	}
	getContext() {
		return {
			imageSmoothingEnabled: false,
			imageSmoothingQuality: "low",
			clearRect: () => {},
			drawImage: (from: FakeCanvas, _x: number, _y: number, width: number, height: number) => {
				this.draws.push({ from, width, height, into: this });
			},
		};
	}
}

const canvas = (c: FakeCanvas) => c as unknown as HTMLCanvasElement;

describe("minificationFor", () => {
	it("draws the canvas itself, unsmoothed, at or above one device pixel per pixel", () => {
		expect(minificationFor(1, 4096, 3328)).toEqual({ level: 0, smooth: false });
		expect(minificationFor(3.5, 4096, 3328)).toEqual({ level: 0, smooth: false });
	});

	it("draws from the halving nearest the size on screen", () => {
		expect(minificationFor(0.9999999, 4096, 3328)).toEqual({ level: 0, smooth: true });
		expect(minificationFor(0.75, 4096, 3328)).toEqual({ level: 0, smooth: true });
		expect(minificationFor(0.7, 4096, 3328)).toEqual({ level: 1, smooth: true });
		expect(minificationFor(0.51, 4096, 3328)).toEqual({ level: 1, smooth: true });
		expect(minificationFor(0.4999999, 4096, 3328)).toEqual({ level: 1, smooth: true });
		expect(minificationFor(0.36, 4096, 3328)).toEqual({ level: 1, smooth: true });
		expect(minificationFor(0.35, 4096, 3328)).toEqual({ level: 2, smooth: true });
		expect(minificationFor(0.17, 4096, 3328)).toEqual({ level: 3, smooth: true });
		expect(minificationFor(0.1, 4096, 3328)).toEqual({ level: 3, smooth: true });
	});

	it("never halves the shorter side below one pixel", () => {
		expect(minificationFor(0.001, 8, 4096)).toEqual({ level: 3, smooth: true });
		expect(minificationFor(0.01, 1, 1)).toEqual({ level: 0, smooth: true });
	});

	it("leaves an unknown scale alone", () => {
		expect(minificationFor(0, 512, 512)).toEqual({ level: 0, smooth: false });
		expect(minificationFor(Number.NaN, 512, 512)).toEqual({ level: 0, smooth: false });
	});
});

describe("levelSize", () => {
	it("rounds odd sizes up at each halving", () => {
		expect(levelSize(3328, 4096, 2)).toEqual({ width: 832, height: 1024 });
		expect(levelSize(5, 3, 1)).toEqual({ width: 3, height: 2 });
		expect(levelSize(5, 3, 4)).toEqual({ width: 1, height: 1 });
	});
});

describe("MinifiedCanvas", () => {
	it("halves the source once per level into the target and releases the steps between", () => {
		const draws: Draw[] = [];
		const scratch: FakeCanvas[] = [];
		const minifier = new MinifiedCanvas({
			alpha: false,
			createCanvas: () => {
				const c = new FakeCanvas(300, 150, draws);
				scratch.push(c);
				return canvas(c);
			},
		});
		const source = new FakeCanvas(3328, 4096, draws);
		const target = new FakeCanvas(300, 150, draws);

		minifier.update(canvas(source), canvas(target), 3);

		expect(draws.map(({ width, height }) => [width, height])).toEqual([[1664, 2048], [832, 1024], [416, 512]]);
		expect(draws[0].from).toBe(source);
		expect(draws[1].from).toBe(draws[0].into);
		expect(draws[2].from).toBe(draws[1].into);
		expect(draws[2].into).toBe(target);
		expect([target.width, target.height]).toEqual([416, 512]);
		expect(scratch.every((c) => c.width === 0 && c.height === 0)).toBe(true);
	});

	it("empties the target at level 0 and for an empty source", () => {
		const minifier = new MinifiedCanvas({ alpha: true, createCanvas: () => canvas(new FakeCanvas()) });
		const target = new FakeCanvas(416, 512);
		minifier.update(canvas(new FakeCanvas(3328, 4096)), canvas(target), 0);
		expect([target.width, target.height]).toEqual([0, 0]);

		const again = new FakeCanvas(416, 512);
		minifier.update(canvas(new FakeCanvas(0, 0)), canvas(again), 2);
		expect([again.width, again.height]).toEqual([0, 0]);
	});
});
