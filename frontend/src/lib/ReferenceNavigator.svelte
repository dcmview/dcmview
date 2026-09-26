<script lang="ts">
	import {
		fetchReferences,
		type FileSummary,
		type ReferenceCatalogResponse,
	} from "../api";
	import {
		ensureWhenSettled,
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import ReferenceEdge from "./ReferenceEdge.svelte";

	let {
		fileIndex,
		files,
		onopenreference,
	}: {
		fileIndex: number;
		files: FileSummary[];
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

	$effect(() => ensureWhenSettled(resources, fileIndex));

	function retry() {
		void resources.reload(fileIndex).catch(() => {});
	}
</script>

<section class="reference-navigator" aria-label="DICOM references">
	<header>
		<span class="title">References</span>
		{#if !activeResource || activeResource.status === "loading"}
			<span class="status">Loading…</span>
		{:else if activeResource?.status === "error"}
			<span class="status error" title={activeResource.error ?? undefined}>Unavailable</span>
			<button class="retry" type="button" onclick={retry}>Retry</button>
		{:else}
			<span class="count">{references.length}</span>
		{/if}
	</header>

	{#if activeResource?.status === "ready" && references.length === 0}
		<span class="empty">No typed references</span>
	{:else if references.length > 0}
		<div class="edges">
			{#each references as reference, referenceIndex (`${reference.relationship}:${referenceIndex}`)}
				<ReferenceEdge {reference} {files} {onopenreference} inline />
			{/each}
		</div>
	{/if}
</section>

<style>
	.reference-navigator {
		display: flex;
		align-items: stretch;
		gap: 0.55rem;
		min-width: 0;
		min-height: 2.1rem;
		padding: 0.3rem 0.55rem;
		border-bottom: 1px solid var(--border-subtle);
		background: var(--surface-chrome);
		color: var(--text-secondary);
		font-size: 0.72rem;
	}

	header {
		display: flex;
		flex: 0 0 auto;
		align-items: center;
		gap: 0.35rem;
	}

	.title {
		font-weight: 650;
		color: var(--text-primary);
	}

	.count {
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
	}

	.edges {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		min-width: 0;
		overflow-x: auto;
	}

	.retry {
		padding: 0.1rem 0.3rem;
		border: 1px solid var(--border-strong);
		border-radius: 3px;
		background: var(--surface-panel);
		color: var(--text-primary);
		font: inherit;
		cursor: pointer;
	}

	.retry:hover {
		border-color: var(--accent);
	}

	.error {
		color: var(--danger);
	}

	.empty,
	.status {
		align-self: center;
		color: var(--text-muted);
	}

	@media (max-width: 519px) {
		.reference-navigator {
			gap: 0.35rem;
			padding-inline: 0.4rem;
		}
	}
</style>
