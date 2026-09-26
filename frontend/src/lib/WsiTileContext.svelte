<script lang="ts">
	import { untrack } from "svelte";
	import { fetchWsiFrameContext, type WsiFrameContextResponse } from "../api";
	import {
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import { wsiMinimapGeometry } from "./wsiMinimap";

	type TileKey = `${number}:${number}`;

	let { fileIndex, frame }: { fileIndex: number; frame: number } = $props();
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
</script>

<section class="wsi-context" aria-label="Whole slide tile position">
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
			<span class="warning">Tile position unavailable: {error}</span>
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
		<div class="warning">{context.warnings.join(" · ") || "Slide-position metadata is missing or invalid."}</div>
	{/if}
	{#if context}
		<div class="boundary">Selected tile only · no stitching or Total Pixel Matrix reconstruction</div>
	{/if}
</section>

<style>
	.wsi-context { display: flex; justify-content: space-between; gap: 14px; padding: 8px 12px; border-bottom: 1px solid var(--border-subtle); background: var(--surface-panel); color: var(--text-secondary); font-size: 11px; }
	.labels { display: flex; flex-wrap: wrap; align-content: flex-start; gap: 4px 12px; }
	.labels strong { width: 100%; color: var(--text-primary); font-size: 12px; }
	.labels span { font-family: var(--font-mono); }
	.minimap-wrap { display: grid; justify-items: end; gap: 2px; color: var(--text-muted); font-family: var(--font-mono); }
	.minimap { overflow: visible; }
	.matrix { fill: var(--surface-viewport); stroke: var(--border-strong); }
	.tile { fill: var(--accent); stroke: var(--accent-text); vector-effect: non-scaling-stroke; }
	.warning { color: var(--danger-text); }
	.boundary { align-self: flex-end; color: var(--text-muted); white-space: nowrap; }
	@media (max-width: 850px) { .wsi-context { flex-wrap: wrap; } .boundary { white-space: normal; } }
</style>
