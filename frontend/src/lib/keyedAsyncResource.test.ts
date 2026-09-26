import { describe, expect, it, vi } from "vitest";
import { KeyedAsyncResource } from "./keyedAsyncResource";

function deferred<Value>() {
	let resolve!: (value: Value) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<Value>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

async function flushPromises(): Promise<void> {
	await Promise.resolve();
	await Promise.resolve();
}

describe("KeyedAsyncResource", () => {
	it("deduplicates an in-flight request for the same key", async () => {
		const request = deferred<string>();
		const load = vi.fn(() => request.promise);
		const resource = new KeyedAsyncResource<number, string>({ load });

		const first = resource.ensure(4);
		const second = resource.ensure(4);
		expect(first).toBe(second);
		expect(load).toHaveBeenCalledTimes(1);

		request.resolve("tags");
		await expect(first).resolves.toBe("tags");
		expect(resource.get(4)).toMatchObject({ status: "ready", value: "tags" });
	});

	it("ignores an older generation that completes after a reload", async () => {
		const oldRequest = deferred<string>();
		const newRequest = deferred<string>();
		const load = vi.fn()
			.mockReturnValueOnce(oldRequest.promise)
			.mockReturnValueOnce(newRequest.promise);
		const resource = new KeyedAsyncResource<number, string>({ load });

		const oldPromise = resource.ensure(9);
		const newPromise = resource.reload(9);
		newRequest.resolve("new");
		await newPromise;
		oldRequest.resolve("old");
		await oldPromise;

		expect(resource.get(9)).toMatchObject({
			status: "ready",
			value: "new",
			generation: 2,
		});
	});

	it("keeps failures and loading state isolated by key", async () => {
		const success = deferred<string>();
		const load = vi.fn((key: number) => key === 1
			? Promise.reject(new Error("file one failed"))
			: success.promise);
		const resource = new KeyedAsyncResource<number, string>({ load });

		void resource.ensure(1).catch(() => {});
		void resource.ensure(2);
		await flushPromises();

		expect(resource.get(1)).toMatchObject({
			status: "error",
			error: "file one failed",
		});
		expect(resource.get(2)).toMatchObject({
			status: "loading",
			error: null,
		});

		success.resolve("file two tags");
		await flushPromises();
		expect(resource.get(2)).toMatchObject({
			status: "ready",
			value: "file two tags",
		});
	});

	it("aborts in-flight loads for other keys and ignores their late results", async () => {
		const requests = new Map<number, ReturnType<typeof deferred<string>>>();
		const signals = new Map<number, AbortSignal>();
		const load = vi.fn((key: number, signal: AbortSignal) => {
			const request = deferred<string>();
			requests.set(key, request);
			signals.set(key, signal);
			return request.promise;
		});
		const resource = new KeyedAsyncResource<number, string>({ load });

		void resource.ensure(1).catch(() => {});
		void resource.ensure(2);
		resource.abortOthers(2);

		expect(signals.get(1)?.aborted).toBe(true);
		expect(signals.get(2)?.aborted).toBe(false);
		expect(resource.get(1).status).toBe("idle");

		requests.get(1)!.resolve("stale");
		requests.get(2)!.resolve("current");
		await flushPromises();
		expect(resource.get(1)).toMatchObject({ status: "idle", value: undefined });
		expect(resource.get(2)).toMatchObject({ status: "ready", value: "current" });

		void resource.ensure(1);
		expect(load).toHaveBeenCalledTimes(3);
	});

	it("keeps only the most recently used settled values within capacity", async () => {
		const resource = new KeyedAsyncResource<number, string>({
			load: async (key) => `value ${key}`,
			capacity: 2,
		});

		await resource.ensure(1);
		await resource.ensure(2);
		await resource.ensure(1);
		await resource.ensure(3);

		expect(resource.get(1)).toMatchObject({ status: "ready", value: "value 1" });
		expect(resource.get(2)).toMatchObject({ status: "idle", value: undefined });
		expect(resource.get(3)).toMatchObject({ status: "ready", value: "value 3" });
	});
});
