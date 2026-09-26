import { describe, expect, it, vi } from "vitest";
import type { RawFrame } from "../../rawFrame";
import type { WlRendererRequest, WlRendererResponse } from "../workers/wlRendererProtocol";
import { WlRendererClient, type WlRenderRequest } from "./wlRendererClient";

function rawFrame(rows = 2, columns = 2): RawFrame {
	return {
		buffer: new Uint8Array(rows * columns).map((_, index) => index * 10).buffer,
		metadata: {
			rows,
			columns,
			bitsAllocated: 8,
			pixelRepresentation: 0,
			samplesPerPixel: 1,
			photometricInterpretation: "MONOCHROME2",
			rescaleSlope: 1,
			rescaleIntercept: 0,
			defaultWc: null,
			defaultWw: null,
			paddingLow: null,
			paddingHigh: null,
		},
	};
}

type FakeContext = {
	drawImage: ReturnType<typeof vi.fn>;
	putImageData: ReturnType<typeof vi.fn>;
	createImageData: (width: number, height: number) => { data: Uint8ClampedArray };
};

function fakeCanvas(): HTMLCanvasElement & { ctx: FakeContext } {
	const ctx: FakeContext = {
		drawImage: vi.fn(),
		putImageData: vi.fn(),
		createImageData: (width, height) => ({ data: new Uint8ClampedArray(width * height * 4) }),
	};
	return { width: 0, height: 0, ctx, getContext: () => ctx } as unknown as HTMLCanvasElement & { ctx: FakeContext };
}

class FakeWorker {
	readonly messages: WlRendererRequest[] = [];
	onmessage: ((event: MessageEvent<WlRendererResponse>) => void) | null = null;
	onerror: (() => void) | null = null;
	terminate = vi.fn();

	postMessage(message: WlRendererRequest): void {
		this.messages.push(message);
	}

	renders(): Extract<WlRendererRequest, { type: "render" }>[] {
		return this.messages.filter((message): message is Extract<WlRendererRequest, { type: "render" }> => (
			message.type === "render"
		));
	}

	respond(id: number, width = 2, height = 2): { close: ReturnType<typeof vi.fn> } {
		const bitmap = { width, height, close: vi.fn() };
		this.onmessage?.({
			data: { type: "rendered", id, width, height, bitmap: bitmap as unknown as ImageBitmap },
		} as MessageEvent<WlRendererResponse>);
		return bitmap;
	}

	fail(id: number): void {
		this.onmessage?.({ data: { type: "error", id, message: "boom" } } as MessageEvent<WlRendererResponse>);
	}
}

function workerClient() {
	const worker = new FakeWorker();
	const client = new WlRendererClient({
		createWorker: () => worker as unknown as Worker,
		minWorkerPixels: 1,
	});
	return { worker, client };
}

async function flush(): Promise<void> {
	for (let index = 0; index < 5; index += 1) await Promise.resolve();
}

describe("WlRendererClient", () => {
	it("renders frames below the worker threshold on the main thread", async () => {
		const createWorker = vi.fn();
		const client = new WlRendererClient({ createWorker, minWorkerPixels: 100 });
		const canvas = fakeCanvas();

		await client.render(() => canvas, { frame: rawFrame(2, 3), wc: 20, ww: 40, isCurrent: () => true });

		expect(createWorker).not.toHaveBeenCalled();
		expect([canvas.width, canvas.height]).toEqual([3, 2]);
		expect(canvas.ctx.putImageData).toHaveBeenCalledOnce();
	});

	it("sends a frame to the worker once and only the window per render", async () => {
		const { worker, client } = workerClient();
		const canvas = fakeCanvas();
		const frame = rawFrame();

		const first = client.render(() => canvas, { frame, wc: 1, ww: 2, isCurrent: () => true });
		worker.respond(worker.renders()[0].id);
		await first;
		const second = client.render(() => canvas, { frame, wc: 3, ww: 4, isCurrent: () => true });
		worker.respond(worker.renders()[1].id);
		await second;

		expect(worker.messages.map((message) => message.type)).toEqual(["frame", "render", "render"]);
		expect(worker.renders().map(({ wc, ww }) => [wc, ww])).toEqual([[1, 2], [3, 4]]);
		expect(canvas.ctx.drawImage).toHaveBeenCalledTimes(2);
	});

	it("coalesces renders requested while one is in flight to the newest", async () => {
		const { worker, client } = workerClient();
		const canvas = fakeCanvas();
		const frame = rawFrame();
		const request = (wc: number): WlRenderRequest => ({ frame, wc, ww: 10, isCurrent: () => true });

		const running = client.render(() => canvas, request(1));
		void client.render(() => canvas, request(2));
		void client.render(() => canvas, request(3));
		worker.respond(worker.renders()[0].id);
		await flush();
		worker.respond(worker.renders()[1].id);
		await running;

		expect(worker.renders().map(({ wc }) => wc)).toEqual([1, 3]);
	});

	it("drops a render that was superseded while the worker ran", async () => {
		const { worker, client } = workerClient();
		const canvas = fakeCanvas();
		let current = true;

		const running = client.render(() => canvas, { frame: rawFrame(), wc: 1, ww: 2, isCurrent: () => current });
		current = false;
		const bitmap = worker.respond(worker.renders()[0].id);
		await running;

		expect(bitmap.close).toHaveBeenCalledOnce();
		expect(canvas.ctx.drawImage).not.toHaveBeenCalled();
	});

	it("falls back to the main thread for good after a worker error", async () => {
		const { worker, client } = workerClient();
		const canvas = fakeCanvas();
		const frame = rawFrame();

		const failing = client.render(() => canvas, { frame, wc: 1, ww: 2, isCurrent: () => true });
		worker.fail(worker.renders()[0].id);
		await failing;
		expect(canvas.ctx.putImageData).toHaveBeenCalledOnce();

		await client.render(() => canvas, { frame, wc: 5, ww: 6, isCurrent: () => true });
		expect(worker.renders()).toHaveLength(1);
		expect(canvas.ctx.putImageData).toHaveBeenCalledTimes(2);
	});

	it("rejects outstanding renders and terminates the worker on dispose", async () => {
		const { worker, client } = workerClient();
		const canvas = fakeCanvas();

		const running = client.render(() => canvas, { frame: rawFrame(), wc: 1, ww: 2, isCurrent: () => false });
		client.dispose();
		await running;

		expect(worker.terminate).toHaveBeenCalledOnce();
		expect(canvas.ctx.drawImage).not.toHaveBeenCalled();
		expect(canvas.ctx.putImageData).not.toHaveBeenCalled();
	});
});
