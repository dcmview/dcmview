<script lang="ts">
	import type { GraphicAnnotationCandidate } from "./app/graphicAnnotations.svelte";
	import Button from "./ui/Button.svelte";
	import StatusBadge from "./ui/StatusBadge.svelte";

	let {
		candidates,
		selectedState,
		selectedItem,
		coversFrame,
		ontoggle,
		onstep,
	}: {
		candidates: readonly GraphicAnnotationCandidate[];
		selectedState: number | null;
		/** The annotation item stepped to, or null for every item alike. */
		selectedItem: number | null;
		/** Whether the shown state annotates the displayed frame. */
		coversFrame: boolean;
		ontoggle: (stateFileIndex: number) => void;
		onstep: (step: -1 | 1) => void;
	} = $props();

	const shown = $derived(candidates.find((candidate) => candidate.stateFileIndex === selectedState) ?? null);
	const position = $derived(shown ? shown.items.findIndex((item) => item.index === selectedItem) : -1);
	const itemLabel = $derived.by(() => {
		if (!shown) return "";
		if (position < 0) return `All ${shown.items.length} ${shown.items.length === 1 ? "item" : "items"}`;
		const item = shown.items[position];
		const name = item.texts[0]?.split("\n")[0] ?? item.graphic_types.join(", ");
		return `Item ${position + 1} of ${shown.items.length} · ${name}`;
	});
</script>

<section class="graphic-annotation-bar" aria-label="Presentation state annotations">
	<span class="heading">Annotations</span>
	<div class="choices" role="group" aria-label="Presentation state">
		{#each candidates as candidate (candidate.stateFileIndex)}
			<button
				type="button"
				class:active={candidate.stateFileIndex === selectedState}
				aria-pressed={candidate.stateFileIndex === selectedState}
				title={candidate.detail}
				onclick={() => ontoggle(candidate.stateFileIndex)}
			>
				{candidate.title}
			</button>
		{/each}
	</div>
	{#if shown && shown.items.length > 0}
		<div class="stepper" role="group" aria-label="Annotation item">
			<Button variant="ghost" icon="prev" aria-label="Previous annotation item" title="Previous annotation item (,)" onclick={() => onstep(-1)} />
			<span class="item" aria-live="polite">{itemLabel}</span>
			<Button variant="ghost" icon="next" aria-label="Next annotation item" title="Next annotation item (.)" onclick={() => onstep(1)} />
		</div>
	{/if}
	{#if shown && !coversFrame}
		<StatusBadge status="partial">Not on this frame</StatusBadge>
	{/if}
	{#if shown && shown.skippedObjects > 0}
		<span class="skipped" title="Objects in DISPLAY or MATRIX units, or with malformed data, are not drawn">
			{shown.skippedObjects} {shown.skippedObjects === 1 ? "object" : "objects"} not drawn
		</span>
	{/if}
</section>

<style>
	.graphic-annotation-bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 10px;
		min-width: 0;
		color: var(--ink-muted);
		font: var(--t-meta);
	}

	.heading {
		font: var(--t-micro);
		letter-spacing: 0.06em;
		text-transform: uppercase;
	}

	.choices {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.choices button {
		height: 24px;
		padding: 0 8px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--paper);
		color: var(--ink-muted);
		font: 500 12px/16px var(--font-ui);
		white-space: nowrap;
		cursor: pointer;
	}

	.choices button:hover {
		border-color: var(--subtle);
		color: var(--text);
	}

	.choices button.active {
		border-color: var(--selection-edge);
		background: var(--selection-fill);
		color: var(--text);
	}

	.choices button:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	.stepper {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	.stepper :global(.btn) {
		height: 24px;
		padding: 0 4px;
	}

	.item {
		min-width: 9rem;
		max-width: 22rem;
		overflow: hidden;
		color: var(--text);
		text-align: center;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-variant-numeric: tabular-nums;
	}
</style>
