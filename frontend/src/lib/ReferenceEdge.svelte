<script lang="ts">
	import type { FileSummary, ReferenceSummary } from "../api";
	import {
		referenceDestination,
		referenceDetails,
		referenceIdentity,
	} from "./referenceNavigation";

	/**
	 * One declared reference: its relationship, the declared target identity,
	 * and an Open button for every match that resolves to a loaded frame.
	 * `inline` keeps the edge on one line for the horizontal reference strip;
	 * otherwise it wraps inside a panel.
	 */
	let {
		reference,
		files,
		onopenreference,
		inline = false,
	}: {
		reference: ReferenceSummary;
		files: FileSummary[];
		onopenreference: (fileIndex: number, frameIndex: number) => void;
		inline?: boolean;
	} = $props();
</script>

<div class="edge" class:inline>
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

<style>
	.edge {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.35rem;
		min-width: 0;
		padding-left: 0.45rem;
		border-left: 1px solid var(--border-subtle);
	}

	.edge.inline {
		flex: 0 0 auto;
		flex-wrap: nowrap;
		max-width: min(38rem, 70vw);
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
		color: var(--text-muted);
		white-space: nowrap;
	}

	.unresolved {
		font-style: italic;
	}

	.target {
		padding: 0.15rem 0.4rem;
		border: 1px solid var(--border-strong);
		border-radius: 3px;
		background: var(--surface-panel);
		color: var(--text-primary);
		font: inherit;
		cursor: pointer;
	}

	.target:hover {
		border-color: var(--accent);
	}

	@media (max-width: 519px) {
		.identity {
			max-width: 9rem;
		}
	}
</style>
