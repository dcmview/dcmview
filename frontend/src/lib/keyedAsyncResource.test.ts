import { afterEach, describe, expect, it, vi } from "vitest";
import {
	ensureWhenSettled,
	KeyedAsyncResource,
	METADATA_SETTLE_MS,
	SharedRequestRegistry,
} from "./keyedAsyncResource";

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

function abortable(signal: AbortSignal): Promise<string> {
	return new Promise<string>((_resolve, reject) => {
		signal.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
	});
}

describe("SharedRequestRegistry", () => {
	it("shares one in-flight request between consumers", async () => {
		const request = deferred<string>();
		const load = vi.fn(() => request.promise);
		const registry = new SharedRequestRegistry<string, string>();

		const prefetch = registry.request("4:7", load);
		const foreground = registry.request("4:7", load);

		expect(foreground).toBe(prefetch);
		expect(registry.get("4:7")).toBe(prefetch);
		expect(load).toHaveBeenCalledOnce();

		request.resolve("frame");
		await expect(foreground).resolves.toBe("frame");
		expect(registry.get("4:7")).toBeUndefined();
	});

	it("aborts owned requests only when the registry scope is cleared", async () => {
		let requestSignal!: AbortSignal;
		const registry = new SharedRequestRegistry<string, string>();
		const request = registry.request("2:3", (signal) => {
			requestSignal = signal;
			return abortable(signal);
		});

		expect(requestSignal.aborted).toBe(false);
		registry.abortAll();
		expect(requestSignal.aborted).toBe(true);
		await expect(request).rejects.toMatchObject({ name: "AbortError" });
		expect(registry.get("2:3")).toBeUndefined();
	});

	it("aborts one key or every other key and starts fresh afterwards", async () => {
		const signals = new Map<string, AbortSignal>();
		const registry = new SharedRequestRegistry<string, string>();
		const load = (key: string) => (signal: AbortSignal) => {
			signals.set(key, signal);
			return abortable(signal);
		};
		const first = registry.request("a", load("a"));
		void registry.request("b", load("b")).catch(() => {});
		void registry.request("c", load("c")).catch(() => {});

		expect(registry.abortOthers("a")).toEqual(["b", "c"]);
		expect([signals.get("a")?.aborted, signals.get("b")?.aborted, signals.get("c")?.aborted])
			.toEqual([false, true, true]);

		registry.abort("a");
		await expect(first).rejects.toMatchObject({ name: "AbortError" });
		const second = registry.request("a", load("a"));
		expect(second).not.toBe(first);
		expect(registry.get("a")).toBe(second);
		registry.abortAll();
		await expect(second).rejects.toMatchObject({ name: "AbortError" });
	});
});

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

	it("aborts the superseded request when reloading", () => {
		const signals: AbortSignal[] = [];
		const resource = new KeyedAsyncResource<number, string>({
			load: (_key, signal) => {
				signals.push(signal);
				return abortable(signal);
			},
		});

		void resource.ensure(3).catch(() => {});
		void resource.reload(3).catch(() => {});

		expect(signals.map((signal) => signal.aborted)).toEqual([true, false]);
		expect(resource.get(3)).toMatchObject({ status: "loading", generation: 2 });
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

describe("ensureWhenSettled", () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it("loads a key only after it stays selected for the settle delay", async () => {
		vi.useFakeTimers();
		const load = vi.fn(async (key: number) => `tags ${key}`);
		const resource = new KeyedAsyncResource<number, string>({ load });

		const cancelFirst = ensureWhenSettled(resource, 1);
		await vi.advanceTimersByTimeAsync(METADATA_SETTLE_MS - 1);
		cancelFirst?.();
		ensureWhenSettled(resource, 2);
		await vi.advanceTimersByTimeAsync(METADATA_SETTLE_MS);

		expect(load.mock.calls.map(([key]) => key)).toEqual([2]);
		expect(resource.get(2)).toMatchObject({ status: "ready", value: "tags 2" });
	});

	it("serves ready keys without waiting and aborts loads for keys left behind", async () => {
		vi.useFakeTimers();
		const signals = new Map<number, AbortSignal>();
		const resource = new KeyedAsyncResource<number, string>({
			load: (key, signal) => {
				signals.set(key, signal);
				return key === 1 ? Promise.resolve("ready") : abortable(signal);
			},
		});
		await resource.ensure(1);
		void resource.ensure(2).catch(() => {});

		expect(ensureWhenSettled(resource, 1)).toBeUndefined();
		expect(signals.get(2)?.aborted).toBe(true);
		expect(resource.get(2).status).toBe("idle");
	});
});
