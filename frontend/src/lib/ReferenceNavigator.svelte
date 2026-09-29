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
	import Button from "./ui/Button.svelte";

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

	export function retryFailedLoads(): void {
		const key = fileIndex;
		if (resources.get(key).status === "error") void resources.reload(key).catch(() => {});
	}
</script>

<!-- Shown only when there is something to act on: references, or a failed load to retry. -->
{#if activeResource?.status === "error"}
	<section class="reference-navigator" aria-label="DICOM references">
		<span class="title">References</span>
		<span class="error" title={activeResource.error ?? undefined}>Unavailable</span>
		<Button icon="reset" onclick={retry}>Retry</Button>
	</section>
{:else if references.length > 0}
	<section class="reference-navigator" aria-label="DICOM references">
		<span class="title">References</span>
		<span class="count">{references.length}</span>
		<div class="edges">
			{#each references as reference, referenceIndex (`${reference.relationship}:${referenceIndex}`)}
				<ReferenceEdge {reference} {files} {onopenreference} inline />
			{/each}
		</div>
	</section>
{/if}

<style>
	.reference-navigator {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
		margin-left: auto;
		color: var(--ink-muted);
		font: var(--t-meta);
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

	.edges {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
		overflow-x: auto;
	}

	.error {
		color: var(--red-text);
	}
</style>
