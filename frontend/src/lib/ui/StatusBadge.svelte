<script lang="ts">
	import type { Snippet } from "svelte";
	import Icon from "./Icon.svelte";
	import type { IconName } from "./icons";

	export type Status = "positive" | "negative" | "progress" | "partial" | "unknown";

	/**
	 * Bea's status badge: tint, icon and word together, never colour alone.
	 * The word stays in `text`; the hue is on the edge, icon and tint.
	 */
	let { status, title, children }: { status: Status; title?: string; children: Snippet } = $props();

	const ICON: Record<Status, IconName> = {
		positive: "check",
		negative: "negative",
		progress: "progress",
		partial: "partial",
		unknown: "unknown",
	};
</script>

<span class="badge {status}" {title}>
	<Icon name={ICON[status]} size={12} />
	<span>{@render children()}</span>
</span>

<style>
	.badge {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		height: 20px;
		padding: 0 8px 0 6px;
		box-sizing: border-box;
		border: 1px solid;
		border-radius: var(--radius-pill);
		box-shadow: var(--badge-hi);
		color: var(--text);
		font: 600 11px/14px var(--font-ui);
		white-space: nowrap;
	}

	.positive { background: var(--green-tint); border-color: var(--status-positive); }
	.positive :global(.icon) { stroke: var(--status-positive); }
	.negative { background: var(--red-tint); border-color: var(--status-negative); border-style: dashed; }
	.negative :global(.icon) { stroke: var(--status-negative); }
	.progress { background: var(--blue-tint); border-color: var(--status-progress); }
	.progress :global(.icon) { stroke: var(--status-progress); }
	.partial { background: var(--orange-tint); border-color: var(--status-partial); }
	.partial :global(.icon) { stroke: var(--status-partial); }
	.unknown { background: var(--badge-unknown); border-color: var(--status-unknown); }
	.unknown :global(.icon) { stroke: var(--status-unknown); }
</style>
