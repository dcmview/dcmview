import {
	fetchFiles,
	fetchSeries,
	type FileSummary,
	type FilesResponse,
	type SeriesCatalogResponse,
} from "../../api";
import { indexFilesById, reuseUnchangedEntries } from "../fileRegistry";

const SCANNING_POLL_MS = 500;
/** Polls that change nothing back off to this, e.g. during a long walk. */
const UNCHANGED_POLL_MAX_MS = 2000;
const RETRY_POLL_MS = 1000;

/** Everything a files response can change by: entries only ever append. */
function scanProgress(files: FilesResponse): string {
	return [files.files.length, files.scanned, files.skipped, files.filtered, files.scan_complete].join("|");
}

/**
 * The file and series catalogs. Discovery is progressive, so the catalog is
 * polled until the server reports the scan complete; unchanged entries keep
 * their identity across polls so components do not re-render for them.
 */
export class Catalog {
	// Server payloads are replaced per poll, never mutated: kept raw so
	// reused entries keep their identity and nothing is deep-proxied.
	files = $state.raw<FilesResponse | null>(null);
	series = $state.raw<SeriesCatalogResponse | null>(null);
	/** Set when the first load fails; later failures retry quietly. */
	loadError = $state<string | null>(null);
	/** References can change only when a file arrives or discovery finishes. */
	readonly referenceRevision = $derived(`${this.files?.files.length ?? 0}|${this.files?.scan_complete ?? false}`);
	readonly filesById = $derived<ReadonlyMap<number, FileSummary>>(indexFilesById(this.files?.files ?? []));

	apply(files: FilesResponse, series: SeriesCatalogResponse): void {
		this.series = {
			...series,
			series: reuseUnchangedEntries(this.series?.series, series.series, (entry) => entry.id),
		};
		this.files = {
			...files,
			files: reuseUnchangedEntries(this.files?.files, files.files, (entry) => entry.index),
		};
	}

	/** Polls until the scan completes, calling `onupdate` after each change. Returns stop. */
	poll(onupdate: () => void): () => void {
		let stopped = false;
		let timer: ReturnType<typeof setTimeout> | null = null;
		let progress: string | null = null;
		let delay = SCANNING_POLL_MS;
		const load = async () => {
			try {
				const [files, fetchedSeries] = this.files
					? [await fetchFiles(), null]
					: await Promise.all([fetchFiles(), fetchSeries()]);
				if (stopped) return;
				if (scanProgress(files) === progress) {
					delay = Math.min(delay * 2, UNCHANGED_POLL_MAX_MS);
				} else {
					// The server builds the series catalog from the file list
					// and scan state, so it only changes with them.
					const seriesStale = this.series === null
						|| files.files.length !== this.files?.files.length
						|| files.scan_complete !== this.series.scan_complete;
					const series = fetchedSeries ?? (seriesStale ? await fetchSeries() : this.series);
					if (stopped || !series) return;
					this.apply(files, series);
					progress = scanProgress(files);
					delay = SCANNING_POLL_MS;
					onupdate();
				}
				if (!files.scan_complete || !this.series?.scan_complete) timer = setTimeout(load, delay);
			} catch (error) {
				if (stopped) return;
				if (!this.files) {
					this.loadError = error instanceof Error ? error.message : String(error);
					return;
				}
				timer = setTimeout(load, RETRY_POLL_MS);
			}
		};
		void load();
		return () => {
			stopped = true;
			if (timer !== null) clearTimeout(timer);
		};
	}
}
