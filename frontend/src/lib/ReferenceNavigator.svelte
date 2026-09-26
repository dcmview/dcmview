<script lang="ts">
	import {
		fetchReferences,
		type FileSummary,
		type ReferenceCatalogResponse,
	} from "../api";
	import {
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		METADATA_SETTLE_MS,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import {
		referenceDestination,
		referenceDetails,
		referenceIdentity,
	} from "./referenceNavigation";

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

	$effect(() => {
		const index = fileIndex;
		resources.abortOthers(index);
		if (resources.get(index).status === "ready") {
			void resources.ensure(index);
			return;
		}
		const timer = setTimeout(() => void resources.ensure(index).catch(() => {}), METADATA_SETTLE_MS);
		return () => clearTimeout(timer);
	});

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
				<div class="edge">
					<code>{reference.relationship}</code>
					<span class="identity" title={referenceIdentity(reference.target)}>
						{referenceIdentity(reference.target)}
					</span>
					{#each referenceDetails(reference.target) as detail}
						<span class="detail">{detail}</span>
					{/each}
					{#if reference.matches.length === 0}
						<span class="unresolved">unresolved</span>
					{:else}
						{#each reference.matches as match, matchIndex (`${match.file_index}:${matchIndex}`)}
							{@const destination = referenceDestination(match, files)}
							{#if destination}
								<button
									class="target"
									type="button"
									title={match.path}
									onclick={() => onopenreference(destination.file.index, destination.frameIndex)}
								>
									Open {destination.file.label} · frame {destination.frameIndex + 1}
								</button>
							{:else}
								<span class="unresolved" title={match.path}>local target unavailable</span>
							{/if}
						{/each}
					{/if}
				</div>
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

	header,
	.edge {
		display: flex;
		align-items: center;
		gap: 0.35rem;
	}

	header {
		flex: 0 0 auto;
	}

	.title {
		font-weight: 650;
		color: var(--text-primary);
	}

	.count,
	.detail,
	.unresolved {
		color: var(--text-muted);
	}

	.count {
		font-variant-numeric: tabular-nums;
	}

	.edges {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		min-width: 0;
		overflow-x: auto;
	}

	.edge {
		flex: 0 0 auto;
		max-width: min(38rem, 70vw);
		padding-left: 0.45rem;
		border-left: 1px solid var(--border-subtle);
	}

	code {
		color: var(--accent);
		font-family: var(--font-mono);
		font-size: 0.68rem;
	}

	.identity {
		max-width: 17rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-mono);
	}

	.detail,
	.unresolved {
		white-space: nowrap;
	}

	.unresolved {
		font-style: italic;
	}

	button {
		font: inherit;
	}

	.target,
	.retry {
		border: 1px solid var(--border-strong);
		border-radius: 3px;
		background: var(--surface-panel);
		color: var(--text-primary);
		cursor: pointer;
	}

	.target {
		padding: 0.15rem 0.4rem;
	}

	.retry {
		padding: 0.1rem 0.3rem;
	}

	.target:hover,
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

		.identity {
			max-width: 9rem;
		}
	}
</style>
