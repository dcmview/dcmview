export type ByteBudgetLruOptions<Value> = {
	maxBytes: number;
	sizeOf: (value: Value) => number;
	dispose?: (value: Value) => void;
};

type CacheRecord<Value> = {
	value: Value;
	bytes: number;
};

export class ByteBudgetLruCache<Key, Value> {
	readonly #maxBytes: number;
	readonly #sizeOf: (value: Value) => number;
	readonly #dispose?: (value: Value) => void;
	readonly #entries = new Map<Key, CacheRecord<Value>>();
	#bytes = 0;

	constructor({ maxBytes, sizeOf, dispose }: ByteBudgetLruOptions<Value>) {
		if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) {
			throw new Error("cache maxBytes must be a non-negative safe integer");
		}
		this.#maxBytes = maxBytes;
		this.#sizeOf = sizeOf;
		this.#dispose = dispose;
	}

	get size(): number {
		return this.#entries.size;
	}

	get bytes(): number {
		return this.#bytes;
	}

	get maxBytes(): number {
		return this.#maxBytes;
	}

	has(key: Key): boolean {
		return this.#entries.has(key);
	}

	keys(): IterableIterator<Key> {
		return this.#entries.keys();
	}

	peek(key: Key): Value | undefined {
		return this.#entries.get(key)?.value;
	}

	get(key: Key): Value | undefined {
		const record = this.#entries.get(key);
		if (!record) return undefined;
		this.#entries.delete(key);
		this.#entries.set(key, record);
		return record.value;
	}

	/**
	 * Caches `value`, evicting least-recently-used entries to fit. A value
	 * larger than the whole budget is not cached and stays the caller's.
	 */
	set(key: Key, value: Value): boolean {
		const incomingBytes = this.#measuredBytes(value);
		if (incomingBytes > this.#maxBytes) return false;

		const previous = this.#entries.get(key);
		if (previous) {
			this.#entries.delete(key);
			this.#bytes -= previous.bytes;
			if (previous.value !== value) {
				this.#dispose?.(previous.value);
			}
		}

		while (this.#bytes + incomingBytes > this.#maxBytes) {
			const oldestKey = this.#entries.keys().next().value as Key | undefined;
			if (oldestKey === undefined) break;
			this.delete(oldestKey);
		}

		this.#entries.set(key, { value, bytes: incomingBytes });
		this.#bytes += incomingBytes;
		return true;
	}

	delete(key: Key): boolean {
		const record = this.#entries.get(key);
		if (!record) return false;
		this.#entries.delete(key);
		this.#bytes = Math.max(0, this.#bytes - record.bytes);
		this.#dispose?.(record.value);
		return true;
	}

	clear(): void {
		if (this.#dispose) {
			for (const record of this.#entries.values()) {
				this.#dispose(record.value);
			}
		}
		this.#entries.clear();
		this.#bytes = 0;
	}

	#measuredBytes(value: Value): number {
		const bytes = this.#sizeOf(value);
		if (!Number.isSafeInteger(bytes) || bytes < 0) {
			throw new Error("cache entry size must be a non-negative safe integer");
		}
		return bytes;
	}
}

export type BitmapResource = {
	width: number;
	height: number;
	close: () => void;
};

export function decodedBitmapBytes(bitmap: Pick<BitmapResource, "width" | "height">): number {
	return bitmap.width * bitmap.height * 4;
}

export type DisplayFrameCaches<Bitmap extends BitmapResource = ImageBitmap> = {
	blobs: ByteBudgetLruCache<string, Blob>;
	bitmaps: ByteBudgetLruCache<string, Bitmap>;
};

export function createDisplayFrameCaches<Bitmap extends BitmapResource = ImageBitmap>(
	blobMaxBytes: number,
	bitmapMaxBytes: number,
): DisplayFrameCaches<Bitmap> {
	return {
		blobs: new ByteBudgetLruCache<string, Blob>({
			maxBytes: blobMaxBytes,
			sizeOf: (blob) => blob.size,
		}),
		bitmaps: new ByteBudgetLruCache<string, Bitmap>({
			maxBytes: bitmapMaxBytes,
			sizeOf: decodedBitmapBytes,
			dispose: (bitmap) => bitmap.close(),
		}),
	};
}
