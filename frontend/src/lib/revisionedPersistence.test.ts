import { describe, expect, it, vi } from "vitest";
import { RevisionedPersistenceController } from "./revisionedPersistence";

type Deferred<Value> = {
	promise: Promise<Value>;
	resolve: (value: Value) => void;
	reject: (error: unknown) => void;
};

function deferred<Value>(): Deferred<Value> {
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
	await Promise.resolve();
}

describe("RevisionedPersistenceController", () => {
	it("persists an edit even when initialization and the first edit have equal values", () => {
		const save = vi.fn().mockResolvedValue("first local value");
		const controller = new RevisionedPersistenceController<number, string>({ save });

		controller.initialize(5, "first local value");
		controller.edit(5, "first local value");

		expect(save).toHaveBeenCalledOnce();
		expect(save).toHaveBeenCalledWith(5, "first local value");
	});

	it("serializes writes and keeps a newer edit when the older response completes", async () => {
		const first = deferred<string>();
		const second = deferred<string>();
		const save = vi.fn()
			.mockReturnValueOnce(first.promise)
			.mockReturnValueOnce(second.promise);
		const controller = new RevisionedPersistenceController<number, string>({ save });
		controller.initialize(7, "committed");

		controller.edit(7, "revision one");
		controller.edit(7, "revision two");

		expect(save).toHaveBeenCalledTimes(1);
		expect(controller.get(7)).toMatchObject({
			value: "revision two",
			status: "saving",
		});

		first.resolve("canonical one");
		await flushPromises();

		expect(save).toHaveBeenCalledTimes(2);
		expect(save).toHaveBeenLastCalledWith(7, "revision two");
		// The older response must not replace the newer local edit.
		expect(controller.get(7)).toMatchObject({
			value: "revision two",
			status: "saving",
		});

		second.resolve("canonical two");
		await flushPromises();
		expect(controller.get(7)).toEqual({
			value: "canonical two",
			status: "clean",
			error: null,
		});
	});

	it("keeps files isolated when independent requests complete in reverse order", async () => {
		const firstFile = deferred<string>();
		const secondFile = deferred<string>();
		const save = vi.fn((key: number) => key === 1 ? firstFile.promise : secondFile.promise);
		const controller = new RevisionedPersistenceController<number, string>({ save });
		controller.initialize(1, "one");
		controller.initialize(2, "two");
		controller.edit(1, "one edited");
		controller.edit(2, "two edited");

		secondFile.resolve("two canonical");
		await flushPromises();
		firstFile.resolve("one canonical");
		await flushPromises();

		expect(controller.get(1)).toMatchObject({ status: "clean", value: "one canonical" });
		expect(controller.get(2)).toMatchObject({ status: "clean", value: "two canonical" });
	});

	it("retains failed edits and retries the newest revision", async () => {
		const retry = deferred<string>();
		const save = vi.fn()
			.mockRejectedValueOnce(new Error("network unavailable"))
			.mockReturnValueOnce(retry.promise);
		const controller = new RevisionedPersistenceController<number, string>({ save });
		controller.initialize(3, "old");

		controller.edit(3, "new");
		await flushPromises();
		expect(controller.get(3)).toEqual({
			value: "new",
			status: "error",
			error: "network unavailable",
		});

		controller.retry(3);
		expect(save).toHaveBeenCalledTimes(2);
		expect(save).toHaveBeenLastCalledWith(3, "new");

		retry.resolve("canonical new");
		await flushPromises();
		expect(controller.get(3)).toMatchObject({
			value: "canonical new",
			status: "clean",
			error: null,
		});
	});

	it("rolls a failed edit back to the last committed value", async () => {
		const save = vi.fn()
			.mockResolvedValueOnce("canonical first")
			.mockRejectedValueOnce(new Error("rejected"));
		const controller = new RevisionedPersistenceController<number, string>({ save });
		controller.initialize(11, "server value");
		controller.edit(11, "first edit");
		await flushPromises();
		controller.edit(11, "second edit");
		await flushPromises();
		expect(controller.get(11)?.status).toBe("error");

		controller.rollback(11);
		expect(controller.get(11)).toEqual({
			value: "canonical first",
			status: "clean",
			error: null,
		});
		expect(save).toHaveBeenCalledTimes(2);
	});

	it("reports every state change and keeps the first initialization", () => {
		const onChange = vi.fn();
		const controller = new RevisionedPersistenceController<number, string>({
			save: () => new Promise<string>(() => {}),
			onChange,
		});
		controller.initialize(2, "loaded");
		controller.initialize(2, "late duplicate load");
		expect(controller.get(2)).toEqual({ value: "loaded", status: "clean", error: null });

		controller.edit(2, "edited");
		expect(onChange.mock.calls.map(([, snapshot]) => snapshot.status)).toEqual(["clean", "dirty", "saving"]);
	});

	it("refuses edits before the key has loaded", () => {
		const controller = new RevisionedPersistenceController<number, string>({ save: vi.fn() });
		expect(controller.get(4)).toBeUndefined();
		expect(() => controller.edit(4, "too early")).toThrow("initialized");
	});
});
