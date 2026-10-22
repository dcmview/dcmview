<script lang="ts">
	import { classLegend, formatScore, labelGroups, layerEntries } from "./display";
	import type { NativeAnnotationsResponse } from "./nativeAnnotations";

	let {
		response,
		frame,
		hiddenLayers,
		hiddenClasses,
		onlayertoggle,
		onclasstoggle,
	}: {
		response: NativeAnnotationsResponse;
		/** The zero-based frame shown: frame labels are listed for it only. */
		frame: number;
		/** View state: layers and classes the viewer does not draw. */
		hiddenLayers: ReadonlySet<string>;
		hiddenClasses: ReadonlySet<string>;
		onlayertoggle: (layerId: string) => void;
		onclasstoggle: (classId: string) => void;
	} = $props();

	const uid = $props.id();
	const classes = $derived(classLegend(response));
	const layers = $derived(layerEntries(response));
	const groups = $derived(labelGroups(response, frame, hiddenLayers));

	// Enter and Space press the toggle and go no further: the window would take Space for cine.
	function keepActivationLocal(event: KeyboardEvent): void {
		if (event.key === "Enter" || event.key === " ") event.stopPropagation();
	}

	const shapeCount = (count: number) => `${count} ${count === 1 ? "shape" : "shapes"}`;
</script>

<section class="native-panel" aria-label="Annotations">
	<h3 id={`${uid}-classes`}>Classes</h3>
	{#if classes.length === 0}
		<p class="native-empty">No annotations on this file</p>
	{:else}
		<ul aria-labelledby={`${uid}-classes`}>
			{#each classes as entry (entry.id)}
				{@const shown = !hiddenClasses.has(entry.id)}
				<li>
					<button
						type="button"
						class="native-toggle"
						onkeydown={keepActivationLocal}
						aria-pressed={shown}
						aria-label={`${entry.name}, ${shapeCount(entry.count)}${entry.notDrawn ? `, ${entry.notDrawn} not drawn` : ""}`}
						title={shown ? `Hide ${entry.name}` : `Show ${entry.name}`}
						onclick={() => onclasstoggle(entry.id)}
					>
						<span class="swatch" style:--swatch={entry.color} aria-hidden="true"></span>
						<span class="native-name">{entry.name}</span>
						<span class="native-count">{entry.count}</span>
					</button>
					{#if entry.notDrawn}
						<span class="native-note">{entry.notDrawn} not drawn</span>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}

	{#if layers.length}
		<h3 id={`${uid}-layers`}>Layers</h3>
		<ul aria-labelledby={`${uid}-layers`}>
			{#each layers as entry (entry.id)}
				{@const shown = !hiddenLayers.has(entry.id)}
				<li>
					<button
						type="button"
						class="native-toggle"
						onkeydown={keepActivationLocal}
						aria-pressed={shown}
						aria-label={`${entry.name}, ${shapeCount(entry.count)}${entry.readonly ? ", read-only" : ""}`}
						title={shown ? `Hide ${entry.name}` : `Show ${entry.name}`}
						onclick={() => onlayertoggle(entry.id)}
					>
						<span
							class="swatch"
							class:class-colors={entry.color === null}
							style:--swatch={entry.color}
							aria-hidden="true"
						></span>
						<span class="native-name">{entry.name}</span>
						<span class="native-count">{entry.count}</span>
					</button>
				</li>
			{/each}
		</ul>
	{/if}

	{#if groups.length}
		<h3>Labels</h3>
		{#each groups as group (group.level)}
			<h4 id={`${uid}-labels-${group.level}`}>{group.title}</h4>
			<dl aria-labelledby={`${uid}-labels-${group.level}`}>
				{#each group.labels as label (label.id)}
					<div class="native-label">
						<dt>{label.field}</dt>
						<dd>
							{label.value}{#if label.score !== null}{" "}<span class="native-count">{formatScore(label.score)}</span>{/if}
						</dd>
					</div>
				{/each}
			</dl>
		{/each}
	{/if}
</section>

<style>
	.native-panel {
		padding: 8px 10px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
		color: var(--text);
		font: 400 11px/16px var(--font-ui);
	}

	h3 {
		margin: 8px 0 4px;
		padding-bottom: 4px;
		border-bottom: 1px solid var(--line);
		font: 600 12px/16px var(--font-ui);
	}

	h3:first-child {
		margin-top: 0;
	}

	h4 {
		margin: 6px 0 2px;
		color: var(--ink-muted);
		font: var(--t-micro);
	}

	ul {
		display: grid;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: grid;
	}

	.native-toggle {
		display: flex;
		align-items: center;
		gap: 6px;
		width: 100%;
		padding: 2px 4px;
		margin-inline: -4px;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: none;
		color: inherit;
		font: inherit;
		text-align: start;
		cursor: pointer;
	}

	.native-toggle:hover {
		background: var(--selection-fill);
	}

	.native-toggle:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 1px;
	}

	.native-toggle[aria-pressed="false"] {
		color: var(--ink-muted);
	}

	.native-toggle[aria-pressed="false"] .swatch {
		background: none;
	}

	.swatch {
		flex: none;
		width: 10px;
		height: 10px;
		border: 2px solid var(--swatch);
		border-radius: 2px;
		background: var(--swatch);
	}

	/* A layer without its own color draws each shape in its class color. */
	.swatch.class-colors {
		border-color: var(--ink-muted);
		border-style: dashed;
		background: none;
	}

	.native-name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.native-count,
	.native-note,
	.native-empty {
		color: var(--ink-muted);
		font-variant-numeric: tabular-nums;
	}

	.native-note {
		padding-left: 16px;
	}

	.native-empty {
		margin: 0;
	}

	dl {
		display: grid;
		gap: 1px;
		margin: 0;
	}

	.native-label {
		display: flex;
		gap: 8px;
		justify-content: space-between;
	}

	dt {
		color: var(--ink-muted);
	}

	dd {
		margin: 0;
		text-align: end;
	}
</style>
