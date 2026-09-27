<script lang="ts">
	import type { Snippet } from "svelte";

	/** Joins related icon buttons (orientation, playback) into one control. */
	let { label, children }: { label: string; children: Snippet } = $props();
</script>

<div class="group" role="group" aria-label={label}>
	{@render children()}
</div>

<style>
	.group {
		display: inline-flex;
		flex: none;
	}

	.group > :global(.btn) {
		border-radius: 0;
	}

	.group > :global(.btn + .btn) {
		margin-left: -1px;
	}

	.group > :global(.btn:first-child) {
		border-radius: var(--radius-md) 0 0 var(--radius-md);
	}

	.group > :global(.btn:last-child) {
		border-radius: 0 var(--radius-md) var(--radius-md) 0;
	}
</style>
