export type IndexedFile = {
	index: number;
};

export function indexFilesById<File extends IndexedFile>(
	files: readonly File[],
): ReadonlyMap<number, File> {
	const byId = new Map<number, File>();
	for (const file of files) {
		if (byId.has(file.index)) {
			throw new Error(`duplicate file index ${file.index}`);
		}
		byId.set(file.index, file);
	}
	return byId;
}

export function resolveFilesById<File extends IndexedFile>(
	filesById: ReadonlyMap<number, File>,
	fileIds: readonly number[],
): File[] {
	const resolved: File[] = [];
	for (const fileId of fileIds) {
		const file = filesById.get(fileId);
		if (file) resolved.push(file);
	}
	return resolved;
}

/**
 * Returns `next`, substituting the previous object for every entry whose key
 * and contents are unchanged.
 *
 * The catalog is re-polled while discovery runs. Keeping unchanged entries
 * identical stops every `$derived`/`$effect` that reads the active file or
 * stack from re-running when nothing about it changed.
 */
export function reuseUnchangedEntries<Entry>(
	previous: readonly Entry[] | undefined,
	next: readonly Entry[],
	keyOf: (entry: Entry) => string | number,
): Entry[] {
	if (!previous || previous.length === 0) return [...next];
	const previousByKey = new Map<string | number, Entry>();
	for (const entry of previous) previousByKey.set(keyOf(entry), entry);
	return next.map((entry) => {
		const earlier = previousByKey.get(keyOf(entry));
		return earlier !== undefined && JSON.stringify(earlier) === JSON.stringify(entry)
			? earlier
			: entry;
	});
}
