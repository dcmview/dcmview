<script lang="ts">
	import type { ActiveTool, WlPreset } from './viewerTools';
	import { TOOL_LABELS, TOOL_ORDER, TOOL_SHORTCUTS, WL_PRESETS } from './viewerTools';
	import Button from './ui/Button.svelte';
	import ButtonGroup from './ui/ButtonGroup.svelte';
	import SegmentedControl from './ui/SegmentedControl.svelte';
	import Select from './ui/Select.svelte';
	import type { IconName } from './ui/icons';

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

	const TOOL_ICONS: Record<ActiveTool, IconName> = {
		pan: "pan",
		scroll: "scroll",
		zoom: "zoom",
		window_level: "wl",
		annotate_rect: "roi",
		redact: "redact",
	};
	const toolOptions = TOOL_ORDER.map((tool) => ({
		value: tool,
		label: TOOL_LABELS[tool],
		icon: TOOL_ICONS[tool],
		title: `${TOOL_LABELS[tool]} (${TOOL_SHORTCUTS[tool]})`,
	}));

	function presetLabel(preset: WlPreset): string {
		return preset.ww === undefined ? preset.label : `${preset.label} · W ${preset.ww} C ${preset.wc}`;
	}
</script>

<div class="toolbar">
	<SegmentedControl
		label="Pointer tool"
		options={toolOptions}
		value={activeTool}
		onchange={(tool) => { activeTool = tool; }}
	/>
	<span class="sep"></span>
	<Select
		aria-label="Window preset"
		value={selectedPresetId}
		onchange={(event) => onpresetchange(event.currentTarget.value)}
	>
		{#each WL_PRESETS as preset}
			<option value={preset.id}>{presetLabel(preset)}</option>
		{/each}
	</Select>
	<ButtonGroup label="Orientation">
		<Button icon="flip-h" onclick={onflipH} aria-label="Flip horizontal" title="Flip horizontal" />
		<Button icon="flip-v" onclick={onflipV} aria-label="Flip vertical" title="Flip vertical" />
		<Button icon="rotate-ccw" onclick={onrotateCCW} aria-label="Rotate 90° counter-clockwise" title="Rotate 90° counter-clockwise" />
		<Button icon="rotate-cw" onclick={onrotateCW} aria-label="Rotate 90° clockwise" title="Rotate 90° clockwise" />
	</ButtonGroup>
	<span class="grow"></span>
	<Button icon="export" onclick={onexportAnnotations} title="Export annotations as EMBED CSV">Export ROIs</Button>
	<Button variant="ghost" icon="reset" onclick={onreset} title="Reset viewport (double-click)">Reset view</Button>
</div>

<style>
	.toolbar {
		display: flex;
		align-items: center;
		gap: 10px;
		min-height: var(--bar-h);
		padding: 6px 10px;
		box-sizing: border-box;
		background: var(--paper);
		border-bottom: 1px solid var(--line);
		min-width: 0;
		flex-wrap: wrap;
	}

	.sep {
		width: 1px;
		align-self: stretch;
		margin: 4px 2px;
		background: var(--line);
	}

	.grow {
		flex: 1;
	}
</style>
