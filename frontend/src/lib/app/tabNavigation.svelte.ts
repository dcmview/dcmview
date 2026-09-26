import type { FileSummary, SeriesStackSummary, SeriesSummary } from "../../api";
import {
	findSeriesStackForFile,
	frameAtPosition,
	framePosition,
	navigationFrameAtPosition,
	navigationFramesForFile,
	navigationTabId,
	type NavigationFrameRef,
} from "../seriesNavigation";

/**
 * One open tab: a series stack (every file of a CT/MR series, or a
 * multiframe object) or a lone file, and where the user left it.
 */
export type OpenTab = {
	id: string;
	fileIndex: number;
	currentFrame: number;
	stackPosition: number;
};

export type TabNavigationOptions = {
	series: () => readonly SeriesSummary[];
	files: () => ReadonlyMap<number, FileSummary>;
	/** The active tab changed, including a tab retargeted to its stack. */
	ontabchange?: () => void;
	/** The active source file changed. */
	onfilechange?: (fileIndex: number | null) => void;
};

/**
 * Open tabs and the active logical frame. A tab's frames are its stack's
 * ordered frames, which may span many single-frame files; moving through
 * them changes the active file while the tab (navigation scope) stays.
 */
export class TabNavigation {
	#tabs = $state<OpenTab[]>([]);
	#activeTabId = $state<string | null>(null);
	#activeFileIndex = $state<number | null>(null);
	#currentFrame = $state(0);
	#stackPosition = $state(0);
	readonly #series: () => readonly SeriesSummary[];
	readonly #files: () => ReadonlyMap<number, FileSummary>;
	readonly #ontabchange?: () => void;
	readonly #onfilechange?: (fileIndex: number | null) => void;

	readonly activeFile = $derived.by<FileSummary | null>(() => (
		this.#activeFileIndex === null ? null : this.#files().get(this.#activeFileIndex) ?? null
	));
	readonly activeStack = $derived.by<SeriesStackSummary | null>(() => (
		this.#activeFileIndex === null
			? null
			: findSeriesStackForFile(this.#series(), this.#activeFileIndex)?.stack ?? null
	));
	/** The active tab's ordered logical frames. */
	readonly frames = $derived.by<readonly NavigationFrameRef[]>(() => {
		if (this.activeStack) return this.activeStack.frames;
		if (this.activeFile) return navigationFramesForFile(this.activeFile.index, this.activeFile.frame_count);
		return [];
	});
	/** Keys per-tab view state and viewport caches. */
	readonly scopeKey = $derived(
		this.#activeTabId ?? (this.activeFile ? `file:${this.activeFile.index}` : ""),
	);
	/** Logical frame count per open tab, keyed by the tab's current file. */
	readonly frameCounts = $derived(new Map(this.#tabs.map((tab) => [
		tab.fileIndex,
		this.#stackById(tab.id)?.frames.length ?? this.#files().get(tab.fileIndex)?.frame_count ?? 0,
	])));

	constructor({ series, files, ontabchange, onfilechange }: TabNavigationOptions) {
		this.#series = series;
		this.#files = files;
		this.#ontabchange = ontabchange;
		this.#onfilechange = onfilechange;
	}

	get tabs(): readonly OpenTab[] {
		return this.#tabs;
	}

	get activeTabId(): string | null {
		return this.#activeTabId;
	}

	get activeFileIndex(): number | null {
		return this.#activeFileIndex;
	}

	get currentFrame(): number {
		return this.#currentFrame;
	}

	get stackPosition(): number {
		return this.#stackPosition;
	}

	/** Opens the tab holding `fileIndex` (at that file) or activates it if open. */
	open(fileIndex: number): void {
		const id = navigationTabId(this.#series(), fileIndex);
		const existing = this.#tabs.find((tab) => tab.id === id);
		if (existing) {
			if (this.#activeTabId !== existing.id) this.#saveActiveTab();
			this.#show(existing);
			const stack = this.#stackById(id);
			const position = stack ? framePosition(stack, fileIndex, 0) : null;
			if (position !== null) this.setStackPosition(position);
			return;
		}

		this.#saveActiveTab();
		const next = this.#newTab(fileIndex);
		this.#tabs = [...this.#tabs, next];
		this.#show(next);
	}

	/** Returns to an open tab where the user left it. */
	activate(fileIndex: number): void {
		const target = this.#tabs.find((tab) => tab.fileIndex === fileIndex);
		if (!target) return;
		if (this.#activeTabId !== target.id) this.#saveActiveTab();
		this.#show(target);
	}

	/** Closes a tab; closing the active one activates its neighbour. */
	close(fileIndex: number): void {
		const closingIndex = this.#tabs.findIndex((tab) => tab.fileIndex === fileIndex);
		if (closingIndex === -1) return;
		const closingId = this.#tabs[closingIndex].id;
		const wasActive = this.#activeTabId === closingId;
		const remaining = this.#tabs.filter((tab) => tab.id !== closingId);
		this.#tabs = remaining;
		if (!wasActive) return;
		this.#show(remaining[Math.min(closingIndex, remaining.length - 1)] ?? null);
	}

	/** Moves the active tab to a logical frame position (clamped). */
	setStackPosition(position: number): void {
		const frame = navigationFrameAtPosition(this.frames, position);
		if (!frame) return;
		this.#moveTo(frame.file_index, frame.frame_index, frame.virtual_index);
	}

	/** Opens a validated reference target at its frame. */
	openReference(fileIndex: number, frameIndex: number): void {
		const file = this.#files().get(fileIndex);
		if (!file || !Number.isInteger(frameIndex) || frameIndex < 0 || frameIndex >= file.frame_count) return;

		this.open(fileIndex);
		const stack = this.#stackById(this.#activeTabId);
		const position = stack ? framePosition(stack, fileIndex, frameIndex) : null;
		if (position !== null) {
			this.setStackPosition(position);
			return;
		}
		this.#moveTo(fileIndex, frameIndex, frameIndex);
	}

	/**
	 * Reconciles tabs with a newer series catalog: the first file opens when
	 * nothing is open, and a lone-file tab whose file joined a stack becomes
	 * that stack's tab at the same frame.
	 */
	syncCatalog(firstFileIndex: number | null): void {
		const activeFileIndex = this.#activeFileIndex;
		if (activeFileIndex === null && this.#tabs.length === 0 && firstFileIndex !== null) {
			this.open(firstFileIndex);
			return;
		}
		if (activeFileIndex === null || this.#activeTabId === null) return;
		const located = findSeriesStackForFile(this.#series(), activeFileIndex);
		if (located && this.#activeTabId !== located.stack.id) {
			const previousId = this.#activeTabId;
			this.#tabs = this.#tabs.map((tab) => tab.id === previousId ? { ...tab, id: located.stack.id } : tab);
			this.#activeTabId = located.stack.id;
			this.#ontabchange?.();
		}
		const position = located ? framePosition(located.stack, activeFileIndex, this.#currentFrame) : null;
		if (position !== null) {
			this.#stackPosition = position;
			this.#updateActiveTab({ stackPosition: position });
		}
	}

	#stackById(id: string | null): SeriesStackSummary | null {
		if (id === null) return null;
		for (const series of this.#series()) {
			const stack = series.stacks.find((candidate) => candidate.id === id);
			if (stack) return stack;
		}
		return null;
	}

	#newTab(fileIndex: number): OpenTab {
		const series = this.#series();
		const located = findSeriesStackForFile(series, fileIndex);
		const position = located ? framePosition(located.stack, fileIndex, 0) ?? 0 : 0;
		const frame = located ? frameAtPosition(located.stack, position) : null;
		return {
			id: navigationTabId(series, fileIndex),
			fileIndex: frame?.file_index ?? fileIndex,
			currentFrame: frame?.frame_index ?? 0,
			stackPosition: position,
		};
	}

	#saveActiveTab(): void {
		if (this.#activeTabId === null || this.#activeFileIndex === null) return;
		this.#updateActiveTab({
			fileIndex: this.#activeFileIndex,
			currentFrame: this.#currentFrame,
			stackPosition: this.#stackPosition,
		});
	}

	#updateActiveTab(change: Partial<Omit<OpenTab, "id">>): void {
		const id = this.#activeTabId;
		if (id === null) return;
		this.#tabs = this.#tabs.map((tab) => tab.id === id ? { ...tab, ...change } : tab);
	}

	#moveTo(fileIndex: number, currentFrame: number, stackPosition: number): void {
		const fileChanged = fileIndex !== this.#activeFileIndex;
		this.#stackPosition = stackPosition;
		this.#activeFileIndex = fileIndex;
		this.#currentFrame = currentFrame;
		this.#updateActiveTab({ fileIndex, currentFrame, stackPosition });
		if (fileChanged) this.#onfilechange?.(fileIndex);
	}

	#show(tab: OpenTab | null): void {
		const tabChanged = (tab?.id ?? null) !== this.#activeTabId;
		const fileIndex = tab?.fileIndex ?? null;
		const fileChanged = fileIndex !== this.#activeFileIndex;
		this.#activeTabId = tab?.id ?? null;
		this.#activeFileIndex = fileIndex;
		this.#currentFrame = tab?.currentFrame ?? 0;
		this.#stackPosition = tab?.stackPosition ?? 0;
		if (tabChanged) this.#ontabchange?.();
		if (fileChanged) this.#onfilechange?.(fileIndex);
	}
}
