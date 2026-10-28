<script lang="ts">
	import { untrack } from "svelte";
	import {
		fetchReferences,
		type FileSummary,
		type ReferenceCatalogResponse,
	} from "../api";
	import {
		ensureWhenSettled,
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		METADATA_SETTLE_MS,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import ReferenceEdge from "./ReferenceEdge.svelte";
	import Button from "./ui/Button.svelte";
	import Icon from "./ui/Icon.svelte";

	let {
		fileIndex,
		files,
		scanProgress,
		onopenreference,
	}: {
		fileIndex: number;
		files: FileSummary[];
		/** Catalog discovery counters and completion, independent of the active file. */
		scanProgress: string;
		onopenreference: (fileIndex: number, frameIndex: number) => void;
	} = $props();

	let resourcesByFile = $state<Record<number, AsyncResourceSnapshot<ReferenceCatalogResponse> | undefined>>({});
	const resources = new KeyedAsyncResource<number, ReferenceCatalogResponse>({
		load: fetchReferences,
		capacity: METADATA_CACHE_FILES,
		onChange: (index, snapshot) => {
			const { [index]: _previous, ...rest } = resourcesByFile;
			resourcesByFile = snapshot.status === "idle" ? rest : { ...rest, [index]: snapshot };
		},
	});
	const activeResource = $derived(resourcesByFile[fileIndex]);
	const references = $derived(activeResource?.value?.references ?? []);

	const loadedProgress = new Map<number, string>();
	$effect(() => {
		const index = fileIndex;
		const progress = scanProgress;
		return untrack(() => {
			if (loadedProgress.get(index) === progress) return ensureWhenSettled(resources, index);
			resources.abortOthers(index);
			const timer = setTimeout(() => {
				loadedProgress.set(index, progress);
				void resources.reload(index).catch(() => {});
			}, METADATA_SETTLE_MS);
			return () => clearTimeout(timer);
		});
	});

	type Shown = { kind: "none" } | { kind: "references"; count: number } | { kind: "error" };

	/**
	 * What the toolbar button shows. It changes only when the active file's
	 * load settles, so stepping through a series of linked files keeps the
	 * button in place instead of hiding and re-showing it on every file.
	 */
	let lastSettled: Shown = { kind: "none" };
	const shown = $derived.by<Shown>(() => {
		if (activeResource?.status === "error") {
			lastSettled = { kind: "error" };
		} else if (activeResource?.status === "ready") {
			lastSettled = references.length > 0 ? { kind: "references", count: references.length } : { kind: "none" };
		}
		return lastSettled;
	});
	const settled = $derived(activeResource?.status === "ready" || activeResource?.status === "error");

	let open = $state(false);
	let root = $state<HTMLElement | null>(null);
	let toggle = $state<HTMLButtonElement | null>(null);
	const popoverId = `reference-popover-${Math.random().toString(36).slice(2, 10)}`;

	$effect(() => {
		if (shown.kind === "none") open = false;
	});

	function close(restoreFocus: boolean): void {
		open = false;
		if (restoreFocus) toggle?.focus();
	}

	function closeOnOutsidePointer(event: PointerEvent): void {
		if (open && root && !root.contains(event.target as Node)) close(false);
	}

	function handleKeydown(event: KeyboardEvent): void {
		if (event.key !== "Escape" || !open) return;
		event.stopPropagation();
		event.preventDefault();
		close(true);
	}

	function openReference(fileIndex: number, frameIndex: number): void {
		close(false);
		onopenreference(fileIndex, frameIndex);
	}

	function retry() {
		void resources.reload(fileIndex).catch(() => {});
	}

	export function retryFailedLoads(): void {
		const key = fileIndex;
		if (resources.get(key).status === "error") void resources.reload(key).catch(() => {});
	}
</script>

<svelte:document onpointerdown={closeOnOutsidePointer} onkeydown={handleKeydown} />

<!--
	A toolbar control, not a strip above the image: its slot is always laid
	out, so references arriving after the frame never resize the viewport.
	It is hidden (but keeps its place) while the active file links to nothing.
-->
<div class="reference-navigator" class:hidden={shown.kind === "none"} bind:this={root}
	aria-hidden={shown.kind === "none" ? "true" : undefined}>
	<Button
		icon="references"
		bind:element={toggle}
		onclick={() => open = !open}
		disabled={shown.kind === "none"}
		aria-label={shown.kind === "error"
			? "References unavailable"
			: `References${shown.kind === "references" ? ` (${shown.count})` : ""}`}
		aria-haspopup="dialog"
		aria-expanded={open}
		aria-controls={popoverId}
		title="References to other files"
	>
		{#if shown.kind === "error"}
			<span class="badge error-badge" aria-hidden="true">!</span>
		{:else}
			<span class="badge" class:stale={!settled} aria-hidden="true">{shown.kind === "references" ? shown.count : 0}</span>
		{/if}
	</Button>
	{#if open}
		<div id={popoverId} class="popover" role="dialog" aria-label="DICOM references">
			<header>
				<span class="title">References</span>
				{#if activeResource?.status === "ready"}
					<span class="count">{references.length}</span>
				{/if}
				<span class="grow"></span>
				<button type="button" class="close" aria-label="Close references" onclick={() => close(true)}>
					<Icon name="close" size={14} />
				</button>
			</header>
			{#if activeResource?.status === "error"}
				<div class="message error">
					<span title={activeResource.error ?? undefined}>References unavailable</span>
					<Button icon="reset" onclick={retry}>Retry</Button>
				</div>
			{:else if !settled}
				<p class="message">Loading references…</p>
			{:else if references.length === 0}
				<p class="message">This file declares no references.</p>
			{:else}
				<div class="edges">
					{#each references as reference, referenceIndex (`${reference.relationship}:${referenceIndex}`)}
						<ReferenceEdge {reference} {files} onopenreference={openReference} />
					{/each}
				</div>
			{/if}
		</div>
	{/if}
</div>

<style>
	.reference-navigator {
		position: relative;
		flex: none;
	}

	.reference-navigator.hidden {
		visibility: hidden;
	}

	.badge {
		min-width: 1.5ch;
		color: var(--text);
		font: var(--t-mono);
		font-variant-numeric: tabular-nums;
		text-align: right;
	}

	.badge.stale {
		color: var(--ink-muted);
	}

	.error-badge {
		color: var(--red-text);
		font-weight: 700;
	}

	.popover {
		position: absolute;
		top: calc(100% + 6px);
		right: 0;
		z-index: 25;
		display: grid;
		grid-template-rows: auto minmax(0, 1fr);
		width: min(34rem, calc(100vw - 24px));
		max-height: min(60vh, 28rem);
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
		color: var(--text);
		font: var(--t-meta);
	}

	header {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 6px 6px 12px;
		border-bottom: 1px solid var(--line);
		color: var(--ink-muted);
	}

	.title {
		font: var(--t-micro);
		letter-spacing: 0.06em;
		text-transform: uppercase;
	}

	.count {
		color: var(--text);
		font: var(--t-mono);
		font-variant-numeric: tabular-nums;
	}

	.grow {
		flex: 1;
	}

	.close {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border: 0;
		border-radius: var(--radius-sm);
		padding: 0;
		background: transparent;
		color: var(--ink-muted);
		cursor: pointer;
	}

	.close:hover {
		color: var(--text);
		background: var(--row-hover);
	}

	.close:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 1px;
	}

	.edges {
		display: grid;
		gap: 10px;
		min-height: 0;
		padding: 10px 12px;
		overflow: auto;
	}

	.message {
		margin: 0;
		padding: 10px 12px;
		color: var(--ink-muted);
	}

	.message.error {
		display: flex;
		align-items: center;
		gap: 8px;
		color: var(--red-text);
	}
</style>
