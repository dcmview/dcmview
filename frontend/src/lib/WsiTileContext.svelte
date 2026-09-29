<script lang="ts">
	import { untrack } from "svelte";
	import {
		fetchWsiFrameContext,
		type FileSummary,
		type WsiFrameContextResponse,
	} from "../api";
	import type { WsiCompanionSummary } from "../generated/api-types";
	import {
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import ReferenceEdge from "./ReferenceEdge.svelte";
	import { wsiMinimapGeometry } from "./wsiMinimap";

	type TileKey = `${number}:${number}`;

	// Image Type value 3 of a WSI instance names its role in the slide.
	const COMPANION_ROLES = [
		["LABEL", "Label"],
		["OVERVIEW", "Overview"],
		["THUMBNAIL", "Thumbnail"],
		["VOLUME", "Pyramid levels"],
	] as const;

	let {
		fileIndex,
		frame,
		files,
		onopenreference,
	}: {
		fileIndex: number;
		frame: number;
		files: FileSummary[];
		onopenreference: (fileIndex: number, frameIndex: number) => void;
	} = $props();
	let snapshotsByTile = $state<Record<TileKey, AsyncResourceSnapshot<WsiFrameContextResponse> | undefined>>({});
	const tiles = new KeyedAsyncResource<TileKey, WsiFrameContextResponse>({
		load: (key) => {
			const [file, tileFrame] = key.split(":").map(Number);
			return fetchWsiFrameContext(file, tileFrame);
		},
		capacity: METADATA_CACHE_FILES,
		onChange: (key, snapshot) => {
			const { [key]: _previous, ...rest } = snapshotsByTile;
			snapshotsByTile = snapshot.status === "idle" ? rest : { ...rest, [key]: snapshot };
		},
	});
	const snapshot = $derived(snapshotsByTile[`${fileIndex}:${frame}`]);
	const context = $derived(snapshot?.status === "ready" ? snapshot.value ?? null : null);
	const error = $derived(snapshot?.status === "error" ? snapshot.error : null);
	const minimap = $derived(wsiMinimapGeometry(context?.total_pixel_matrix ?? null, context?.tile_rectangle ?? null));
	const filesByIndex = $derived(new Map(files.map((file) => [file.index, file])));
	const companionGroups = $derived.by(() => {
		const companions = context?.companions ?? [];
		const known = new Set<string>(COMPANION_ROLES.map(([role]) => role));
		const groups: { title: string; companions: WsiCompanionSummary[] }[] = COMPANION_ROLES.map(([role, title]) => ({
			title,
			companions: companions.filter((companion) => companion.image_type_role === role),
		}));
		groups.push({
			title: "Other",
			companions: companions.filter((companion) => !known.has(companion.image_type_role ?? "")),
		});
		return groups.filter((group) => group.companions.length > 0);
	});

	$effect(() => {
		const key: TileKey = `${fileIndex}:${frame}`;
		untrack(() => {
			tiles.abortOthers(key);
			void tiles.reload(key).catch(() => {});
		});
	});

	function shown(value: string | number | null | undefined): string {
		return value === null || value === undefined || value === "" ? "not declared" : String(value);
	}

	export function retryFailedLoads(): void {
		const key: TileKey = `${fileIndex}:${frame}`;
		if (tiles.get(key).status === "error") void tiles.reload(key).catch(() => {});
	}
</script>

<section class="wsi-context" aria-label="Whole slide tile position">
	<div class="tile-row">
		<div class="labels">
			<strong>Positioned WSI tile</strong>
			{#if context}
				<span>frame {context.frame_index + 1}</span>
				<span>level {shown(context.pyramid_level)}</span>
				<span>row {shown(context.tile_row)} · column {shown(context.tile_column)}</span>
				<span>optical path {shown(context.optical_path?.identifier ?? context.optical_path?.index)}</span>
				<span>focal plane {shown(context.focal_plane?.index)}{context.focal_plane?.z_offset_slide !== null ? ` · z ${context.focal_plane?.z_offset_slide}` : ""}</span>
				<span>{context.tiling_status} tiling · {shown(context.image_type_role)}</span>
			{:else if error}
				<span class="warning status-line">Tile position unavailable: {error}</span>
			{:else}
				<span>Loading tile position…</span>
			{/if}
		</div>
		{#if context && minimap}
			<div class="minimap-wrap">
				<svg
					class="minimap"
					width={minimap.viewWidth}
					height={minimap.viewHeight}
					viewBox={`0 0 ${minimap.viewWidth} ${minimap.viewHeight}`}
					aria-label={`Tile rectangle ${context.tile_rectangle?.x},${context.tile_rectangle?.y} within ${context.total_pixel_matrix?.columns} by ${context.total_pixel_matrix?.rows} matrix`}
				>
					<rect class="matrix" x="0" y="0" width={minimap.viewWidth} height={minimap.viewHeight}></rect>
					<rect class="tile" x={minimap.tile.x} y={minimap.tile.y} width={Math.max(minimap.tile.width, 1)} height={Math.max(minimap.tile.height, 1)}></rect>
				</svg>
				<span>{context.total_pixel_matrix?.columns} × {context.total_pixel_matrix?.rows}</span>
			</div>
		{:else if context}
			<div class="warning status-line">{context.warnings.join(" · ") || "Slide-position metadata is missing or invalid."}</div>
		{/if}
		{#if context}
			<div class="boundary">Selected tile only · no stitching or Total Pixel Matrix reconstruction</div>
		{/if}
	</div>
	{#if context}
		<div class="links">
			<div class="link-group" aria-label="Slide companions">
				<strong>Companions{context.companions_truncated ? ` (first ${context.companions.length})` : ""}</strong>
				{#each companionGroups as group (group.title)}
					<div class="companion-row">
						<span class="role">{group.title}</span>
						{#each group.companions as companion (companion.file_index)}
							{@const file = filesByIndex.get(companion.file_index)}
							{#if file && file.frame_count > 0}
								<button
									class="target"
									type="button"
									title={`${file.path}\nSOP Instance ${companion.sop_instance_uid}`}
									onclick={() => onopenreference(file.index, 0)}
								>
									Open {file.label} · {file.frame_count} {file.frame_count === 1 ? "frame" : "frames"}
								</button>
							{:else}
								<span class="unresolved" title={`SOP Instance ${companion.sop_instance_uid}`}>local target unavailable</span>
							{/if}
						{/each}
					</div>
				{:else}
					<span class="empty">No companion instances share this slide's pyramid or container</span>
				{/each}
			</div>
			<div class="link-group" aria-label="Slide relationships">
				<strong>Relationships{context.relationships_truncated ? ` (first ${context.relationships.length})` : ""}</strong>
				{#each context.relationships as reference, index (`${reference.relationship}:${index}`)}
					<ReferenceEdge {reference} {files} {onopenreference} />
				{:else}
					<span class="empty">No typed references</span>
				{/each}
			</div>
		</div>
	{/if}
</section>

<style>
	.wsi-context { display: grid; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--line); background: var(--paper); color: var(--ink-muted); font: var(--t-meta); font-size: 11px; }
	.tile-row { display: flex; justify-content: space-between; gap: 14px; }
	.labels { display: flex; flex-wrap: wrap; align-content: flex-start; gap: 4px 12px; }
	.labels strong { width: 100%; color: var(--text); font: var(--t-title); font-size: 12px; }
	.labels span { font-family: var(--font-mono); }
	.minimap-wrap { display: grid; justify-items: end; gap: 2px; color: var(--ink-muted); font-family: var(--font-mono); }
	.minimap { overflow: visible; }
	.matrix { fill: var(--surface); stroke: var(--line); }
	.tile { fill: var(--selection-fill); stroke: var(--selection-edge); vector-effect: non-scaling-stroke; }
	.boundary { align-self: flex-end; color: var(--ink-muted); white-space: nowrap; }
	.links { display: grid; grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr)); gap: 6px 16px; max-height: 120px; overflow: auto; }
	.link-group { display: grid; align-content: start; gap: 4px; min-width: 0; }
	.link-group strong { color: var(--ink-muted); font: var(--t-micro); letter-spacing: 0.06em; text-transform: uppercase; }
	.companion-row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 6px; }
	.role { min-width: 6.5rem; color: var(--ink-muted); }
	.target { height: 24px; padding: 0 8px; border: 1px solid var(--ink); border-radius: var(--radius-sm); background: linear-gradient(var(--control-top), var(--control-bot)); box-shadow: var(--elev-control-secondary); color: var(--text); font: 500 12px/16px var(--font-ui); cursor: pointer; }
	.target:hover { background: var(--control-top); }
	.target:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: 2px; }
	.unresolved, .empty { color: var(--ink-muted); font-style: italic; }
	/* Status lines: the status icon in its hue beside text-coloured words (Bea pairs every status with a second channel). */
	.status-line { display: flex; align-items: baseline; gap: 6px; color: var(--text); }
	.status-line::before { content: ""; flex: none; width: 12px; height: 12px; align-self: center; background: var(--status-icon); mask: var(--status-mask) center / contain no-repeat; -webkit-mask: var(--status-mask) center / contain no-repeat; }
	.warning { --status-icon: var(--status-partial); --status-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='black' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round'%3E%3Ccircle cx='8' cy='8' r='5.5'/%3E%3Cpath d='M8 2.5a5.5 5.5 0 0 1 0 11z' fill='black'/%3E%3C/svg%3E"); }
	@media (max-width: 850px) { .tile-row { flex-wrap: wrap; } .boundary { white-space: normal; } }
</style>
