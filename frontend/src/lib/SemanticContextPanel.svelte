<script lang="ts">
	import { untrack } from "svelte";
	import { fetchSemanticContext, type SemanticContextResponse } from "../api";
	import {
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import {
		codedConceptLabel,
		mappingFormula,
		semanticKindLabel,
		semanticModeLabel,
		type SemanticMode,
	} from "./semanticPresentation";

	let {
		fileIndex,
		currentFrame,
		onmodechange,
		oncontextchange,
	}: {
		fileIndex: number;
		currentFrame: number;
		onmodechange?: (mode: SemanticMode) => void;
		oncontextchange?: (response: SemanticContextResponse | null) => void;
	} = $props();
	let snapshotsByFile = $state<Record<number, AsyncResourceSnapshot<SemanticContextResponse> | undefined>>({});
	let mode = $state<SemanticMode>("pixel_preview");
	const contexts = new KeyedAsyncResource<number, SemanticContextResponse>({
		load: (index) => fetchSemanticContext(index),
		capacity: METADATA_CACHE_FILES,
		onChange: (index, snapshot) => {
			const { [index]: _previous, ...rest } = snapshotsByFile;
			snapshotsByFile = snapshot.status === "idle" ? rest : { ...rest, [index]: snapshot };
		},
	});

	const snapshot = $derived(snapshotsByFile[fileIndex]);
	const response = $derived(snapshot?.status === "ready" ? snapshot.value ?? null : null);
	const error = $derived(snapshot?.status === "error" ? snapshot.error : null);
	const loading = $derived(snapshot?.status === "loading");
	const semanticAvailable = $derived(response !== null && response.context.kind !== "not_applicable");
	const currentSegmentMapping = $derived.by(() => {
		if (response?.context.kind !== "segmentation") return null;
		return response.context.frame_mappings.find((mapping) => mapping.frame_index === currentFrame) ?? null;
	});

	// Every visit re-reads the context, which can change while discovery is
	// still resolving the object's references.
	$effect(() => {
		const requestedFile = fileIndex;
		untrack(() => {
			oncontextchange?.(null);
			mode = "pixel_preview";
			onmodechange?.("pixel_preview");
			contexts.abortOthers(requestedFile);
			contexts.reload(requestedFile)
				.then((result) => {
					if (requestedFile === fileIndex) oncontextchange?.(result);
				})
				.catch(() => {});
		});
	});

	function setMode(nextMode: SemanticMode) {
		mode = nextMode;
		onmodechange?.(nextMode);
	}

	function display(value: string | number | null | undefined): string {
		return value === null || value === undefined || value === "" ? "Not declared" : String(value);
	}
</script>

<section class="semantic-panel" aria-label="Object interpretation">
	<header>
		<div>
			<strong>Object interpretation</strong>
			<span>{response ? semanticKindLabel(response.context) : "Inspecting object…"}</span>
		</div>
		<span class="active-mode">Active: {semanticModeLabel(mode)}</span>
	</header>

	<div class="mode-switch" role="group" aria-label="Interpretation mode">
		<button class:active={mode === "pixel_preview"} type="button" onclick={() => setMode("pixel_preview")}>
			Pixel Preview
		</button>
		<button
			class:active={mode === "semantic_context"}
			type="button"
			disabled={!semanticAvailable}
			onclick={() => setMode("semantic_context")}
		>
			Semantic Context
		</button>
	</div>

	{#if loading}
		<p class="message">Loading declared semantic metadata…</p>
	{:else if error}
		<p class="message error">Semantic metadata unavailable: {error}. Pixel Preview remains active.</p>
	{:else if response}
		{#if mode === "pixel_preview"}
			<p class="message">
				Decoded pixels are shown without object-specific alignment or clinical interpretation.
				{response.pixel_preview_preserves_stored_values ? " Stored values are preserved by the raw path." : ""}
			</p>
			{#if response.context.kind === "not_applicable"}
				<p class="reason">Semantic Context unavailable: {response.context.reason}.</p>
			{/if}
		{:else if response.context.kind === "segmentation"}
			<div class="details">
				<div class="summary-grid">
					<span>Segmentation type <b>{display(response.context.segmentation_type)}</b></span>
					<span>Fractional type <b>{display(response.context.segmentation_fractional_type)}</b></span>
					<span>Current frame segment <b>{display(currentSegmentMapping?.segment_number)}</b></span>
				</div>
				{#each response.context.segments as segment (segment.number)}
					<div class="item">
						<strong>Segment {segment.number}: {display(segment.label)}</strong>
						<span>{display(segment.description)}</span>
						<span>Property: {codedConceptLabel(segment.property_type)}</span>
						<span>Algorithm: {display(segment.algorithm_type)} / {display(segment.algorithm_name)}</span>
					</div>
				{/each}
				<p class:eligible={response.context.overlay.eligible} class="reason">
					Overlay {response.context.overlay.eligible ? "eligible" : "unavailable"}: {response.context.overlay.reason}.
				</p>
			</div>
		{:else if response.context.kind === "parametric_map"}
			<div class="details">
				<p class="reason">Canvas values remain stored {response.context.stored_value_type} pixels; declared mappings are shown below and are never inferred.</p>
				{#each response.context.mappings as mapping, index (`${mapping.source}:${mapping.label}:${index}`)}
					<div class="item">
						<strong>{display(mapping.label)} · {mapping.source}</strong>
						<span>{mappingFormula(mapping.slope, mapping.intercept) ?? "LUT mapping"}</span>
						<span>Units: {codedConceptLabel(mapping.units)}</span>
						<span>Quantity: {codedConceptLabel(mapping.quantity)}</span>
						<span>Derivation: {codedConceptLabel(mapping.derivation)}</span>
					</div>
				{:else}
					<p class="reason">No compatible Real World Value Mapping is available.</p>
				{/each}
				{#each response.context.warnings as warning}<p class="reason">{warning}</p>{/each}
			</div>
		{:else if response.context.kind === "rt_dose"}
			<div class="details">
				<div class="summary-grid">
					<span>Dose units <b>{display(response.context.dose_units)}</b></span>
					<span>Dose type <b>{display(response.context.dose_type)}</b></span>
					<span>Summation <b>{display(response.context.dose_summation_type)}</b></span>
					<span>Grid scaling <b>{display(response.context.dose_grid_scaling)}</b></span>
				</div>
				<p class="reason">Scaled value = stored value × {display(response.context.dose_grid_scaling)}. The pixel canvas remains the stored-value preview.</p>
				<p class:eligible={response.context.overlay.eligible} class="reason">
					Overlay {response.context.overlay.eligible ? "eligible" : "unavailable"}: {response.context.overlay.reason}.
				</p>
				<p class="warning">{response.context.clinical_use_warning}</p>
			</div>
		{/if}
	{/if}
</section>

<style>
	.semantic-panel { border-bottom: 1px solid var(--border-subtle); background: var(--surface-chrome); padding: 8px 12px; color: var(--text-secondary); font-size: 12px; }
	header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
	header div { display: flex; align-items: baseline; gap: 8px; }
	header strong { color: var(--text-primary); }
	header span, .active-mode { color: var(--text-muted); }
	.active-mode { font-family: var(--font-mono); }
	.mode-switch { display: flex; gap: 4px; margin-top: 7px; }
	button { border: 1px solid var(--border-strong); border-radius: 4px; padding: 4px 9px; color: var(--text-secondary); background: var(--surface-control); font: inherit; cursor: pointer; }
	button.active { color: var(--surface-root); background: var(--surface-control-active); }
	button:disabled { opacity: .42; cursor: not-allowed; }
	.message, .reason, .warning { margin: 7px 0 0; }
	.error, .warning { color: #ffb4ab; }
	.details { max-height: 180px; overflow: auto; }
	.summary-grid { display: flex; flex-wrap: wrap; gap: 6px 16px; margin-top: 7px; }
	.summary-grid b { color: var(--text-primary); font-family: var(--font-mono); }
	.item { display: grid; gap: 2px; margin-top: 7px; padding: 6px 8px; border-left: 2px solid var(--border-strong); background: var(--surface-panel); }
	.item strong { color: var(--text-primary); }
	.item span { font-family: var(--font-mono); }
	.reason { color: var(--text-muted); }
	.reason.eligible { color: #8bd5a1; }
	@media (max-width: 700px) { header { align-items: flex-start; } .details { max-height: 130px; } }
</style>
