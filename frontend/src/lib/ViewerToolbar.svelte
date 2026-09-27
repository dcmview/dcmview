<script lang="ts">
	import type { ActiveTool } from './viewerTools';
	import { TOOL_LABELS, TOOL_ORDER, TOOL_SHORTCUTS, WL_PRESETS } from './viewerTools';

	let {
		activeTool = $bindable(),
		selectedPresetId,
		onpresetchange,
		onreset,
		onflipH,
		onflipV,
		onrotateCW,
		onrotateCCW,
		onexportAnnotations,
	}: {
		activeTool: ActiveTool;
		selectedPresetId: string;
		onpresetchange: (presetId: string) => void;
		onreset: () => void;
		onflipH: () => void;
		onflipV: () => void;
		onrotateCW: () => void;
		onrotateCCW: () => void;
		onexportAnnotations: () => void;
	} = $props();

	const tools: ActiveTool[] = TOOL_ORDER;
</script>

<div class="toolbar">
	<div class="tool-group">
		{#each tools as tool}
			<button
				type="button"
				class:active={activeTool === tool}
				onclick={() => { activeTool = tool; }}
				title="{TOOL_LABELS[tool]} ({TOOL_SHORTCUTS[tool]})"
			>
				{TOOL_LABELS[tool]}
			</button>
		{/each}
	</div>
	<span class="sep"></span>
	<select value={selectedPresetId} onchange={(event) => onpresetchange(event.currentTarget.value)}>
		{#each WL_PRESETS as preset}
			<option value={preset.id}>{preset.label}</option>
		{/each}
	</select>
	<span class="sep"></span>
	<button type="button" onclick={onexportAnnotations} title="Export annotations as EMBED CSV">Export ROIs</button>
	<span class="sep"></span>
	<button type="button" onclick={onreset} title="Reset viewport (double-click)">Reset</button>
	<div class="tool-group transform-group">
		<button type="button" onclick={onflipH} title="Flip horizontal">↔</button>
		<button type="button" onclick={onflipV} title="Flip vertical">↕</button>
		<span class="sep"></span>
		<button type="button" onclick={onrotateCCW} title="Rotate 90° CCW">↺</button>
		<button type="button" onclick={onrotateCW} title="Rotate 90° CW">↻</button>
	</div>
</div>

<style>
	.toolbar {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		padding: 0.38rem 0.7rem;
		background: var(--surface-chrome);
		border-bottom: 1px solid var(--border-subtle);
		min-width: 0;
		flex-wrap: wrap;
	}
	.tool-group {
		display: flex;
		flex: 0 0 auto;
		padding: 2px;
		background: rgba(255, 255, 255, 0.045);
		border: 1px solid var(--border-subtle);
		border-radius: calc(var(--radius-control) + 2px);
	}
	.transform-group {
		margin-left: auto;
	}
	button {
		min-height: var(--control-height);
		background: transparent;
		border: 1px solid transparent;
		color: var(--text-secondary);
		padding: 0.22rem 0.62rem;
		border-radius: var(--radius-control);
		cursor: pointer;
		font: inherit;
		font-size: 0.82rem;
		line-height: 1;
	}
	button:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}
	.toolbar > button {
		background: var(--surface-control);
		border-color: var(--border-subtle);
	}
	button.active {
		border-color: rgba(255, 255, 255, 0.22);
		color: var(--text-inverse);
		background: var(--surface-control-active);
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
	}
	select {
		min-height: var(--control-height);
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		color: var(--text-primary);
		padding: 0.22rem 1.8rem 0.22rem 0.65rem;
		border-radius: var(--radius-control);
		cursor: pointer;
		font: inherit;
		font-size: 0.82rem;
	}
	button:focus-visible,
	select:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}
	.sep {
		width: 1px;
		height: 1.2rem;
		background: var(--border-subtle);
		margin: 0 0.15rem;
	}

	@media (max-width: 760px) {
		.transform-group {
			margin-left: 0;
		}
	}
</style>
