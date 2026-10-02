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
					title={destination.file.path}
					onclick={() => onopenreference(destination.file.index, destination.frameIndex)}
				>
					Open {destination.file.label} · frame {destination.frameIndex + 1}
				</button>
			{:else}
				<span class="unresolved" title={files.find((file) => file.index === match.file_index)?.path}>local target unavailable</span>
			{/if}
		{/each}
	{/if}
</div>

<style>
	.edge {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		min-width: 0;
		padding-left: 8px;
		border-left: 1px solid var(--line);
		color: var(--text);
		font: var(--t-meta);
	}

	.edge.inline {
		flex: 0 0 auto;
		flex-wrap: nowrap;
		max-width: min(38rem, 70vw);
	}

	code {
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
	}

	.identity {
		max-width: 17rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font: var(--t-mono);
	}

	.detail,
	.unresolved {
		color: var(--ink-muted);
		white-space: nowrap;
	}

	.unresolved {
		font-style: italic;
	}

	.target {
		height: 24px;
		padding: 0 8px;
		border: 1px solid var(--ink);
		border-radius: var(--radius-sm);
		background: linear-gradient(var(--control-top), var(--control-bot));
		box-shadow: var(--elev-control-secondary);
		color: var(--text);
		font: 500 12px/16px var(--font-ui);
		white-space: nowrap;
		cursor: pointer;
	}

	.target:hover {
		background: var(--control-top);
	}

	.target:active {
		transform: translate(1px, 1px);
		box-shadow: var(--press);
	}

	.target:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	@media (max-width: 519px) {
		.identity {
			max-width: 9rem;
		}
	}
</style>
