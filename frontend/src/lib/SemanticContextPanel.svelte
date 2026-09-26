<script lang="ts">
	import { untrack } from "svelte";
	import { fetchSemanticContext, type FileSummary, type SemanticContextResponse } from "../api";
	import {
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import ReferenceEdge from "./ReferenceEdge.svelte";
	import {
		codedConceptLabel,
		formatDeclaredVector,
		gridFrameOffsetSummary,
		mappingFormula,
		rgbCss,
		segmentColorSource,
		semanticKindLabel,
		semanticModeLabel,
		unusedRecommendedColor,
		type SemanticMode,
	} from "./semanticPresentation";

	let {
		fileIndex,
		currentFrame,
		files,
		onopenreference,
		onmodechange,
		oncontextchange,
		onshowoverlay,
	}: {
		fileIndex: number;
		currentFrame: number;
		files: FileSummary[];
		onopenreference: (fileIndex: number, frameIndex: number) => void;
		onmodechange?: (mode: SemanticMode) => void;
		oncontextchange?: (response: SemanticContextResponse | null) => void;
		/** Opens the source image this volume's colorwash is drawn on. */
		onshowoverlay?: (response: SemanticContextResponse) => void;
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
	const activeFile = $derived(files.find((file) => file.index === fileIndex) ?? null);
	const semanticAvailable =$derived(response !== null && response.context.kind !== "not_applicable");
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
					{@const unused = unusedRecommendedColor(segment)}
					<div class="item">
						<strong class="segment-title">
							<span
								class="swatch"
								style:background-color={rgbCss(segment.display_color)}
								title="Overlay color"
								aria-hidden="true"
							></span>
							Segment {segment.number}: {display(segment.label)}
						</strong>
						<span class="recommended">Overlay color: {segmentColorSource(segment)}</span>
						{#if unused}
							<span class="recommended">
								Also recommended:
								{#if unused.color}
									<span class="swatch" style:background-color={rgbCss(unused.color)} aria-hidden="true"></span>
								{/if}
								{unused.text} (not used by the overlay)
							</span>
						{/if}
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
				<p class:eligible={response.context.overlay.eligible} class="reason">
					Overlay {response.context.overlay.eligible ? "eligible" : "unavailable"}: {response.context.overlay.reason}.
				</p>
				{#if response.context.overlay.eligible && onshowoverlay}
					{@const shown = response}
					<button type="button" class="show-overlay" onclick={() => onshowoverlay(shown)}>
						Show map on source image
					</button>
				{/if}
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
				{#if response.context.scaling_status !== "available"}
					<p class="warning">Dose Grid Scaling is {response.context.scaling_status.replace(/_/g, " ")}; values are shown as stored.</p>
				{/if}
				<h3>Dose grid</h3>
				<dl class="geometry">
					<dt>Matrix</dt>
					<dd>{activeFile ? `${activeFile.columns} × ${activeFile.rows} × ${activeFile.frame_count}` : "Not declared"}</dd>
					<dt>Pixel spacing</dt>
					<dd>{display(formatDeclaredVector(response.context.geometry.pixel_spacing))}{response.context.geometry.pixel_spacing ? " mm" : ""}</dd>
					<dt>Frame offsets</dt>
					<dd>{display(gridFrameOffsetSummary(response.context.geometry.grid_frame_offsets))}</dd>
					<dt>Position</dt>
					<dd>{display(formatDeclaredVector(response.context.geometry.image_position_patient))}</dd>
					<dt>Orientation</dt>
					<dd>{display(formatDeclaredVector(response.context.geometry.image_orientation_patient))}</dd>
					<dt>Frame of reference</dt>
					<dd>{display(response.context.geometry.frame_of_reference_uid)}</dd>
				</dl>
				<h3>References</h3>
				<div class="references">
					{#each response.context.references as reference, index (`${reference.relationship}:${index}`)}
						<ReferenceEdge {reference} {files} {onopenreference} />
					{:else}
						<p class="reason">No plan, structure set, or image references are declared.</p>
					{/each}
				</div>
				<p class:eligible={response.context.overlay.eligible} class="reason">
					Overlay {response.context.overlay.eligible ? "eligible" : "unavailable"}: {response.context.overlay.reason}.
				</p>
				{#if response.context.overlay.eligible && onshowoverlay}
					{@const shown = response}
					<button type="button" class="show-overlay" onclick={() => onshowoverlay(shown)}>
						Show dose on source image
					</button>
				{/if}
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
	.error, .warning { color: var(--danger-text); }
	.details { max-height: 180px; overflow: auto; }
	.summary-grid { display: flex; flex-wrap: wrap; gap: 6px 16px; margin-top: 7px; }
	.summary-grid b { color: var(--text-primary); font-family: var(--font-mono); }
	.item { display: grid; gap: 2px; margin-top: 7px; padding: 6px 8px; border-left: 2px solid var(--border-strong); background: var(--surface-panel); }
	.item strong { color: var(--text-primary); }
	.item span { font-family: var(--font-mono); }
	.segment-title, .item .recommended { display: flex; align-items: center; gap: 6px; }
	.swatch { flex: 0 0 auto; width: 10px; height: 10px; border: 1px solid var(--border-strong); border-radius: 2px; }
	h3 { margin: 9px 0 0; color: var(--text-primary); font-size: 11px; font-weight: 650; }
	.geometry { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 2px 12px; margin: 4px 0 0; }
	.geometry dt { color: var(--text-muted); }
	.geometry dd { margin: 0; overflow-wrap: anywhere; color: var(--text-primary); font-family: var(--font-mono); }
	.references { display: grid; gap: 4px; margin-top: 4px; }
	.reason { color: var(--text-muted); }
	.reason.eligible { color: var(--success-text); }
	.show-overlay { margin-top: 5px; }
	@media (max-width: 700px) { header { align-items: flex-start; } .details { max-height: 130px; } }
</style>
