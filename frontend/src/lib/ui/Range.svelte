<script lang="ts">
	import type { HTMLInputAttributes } from "svelte/elements";

	/** Native keyboard and pointer range control using the shared theme roles. */
	let { value = $bindable(0), ...rest }: Omit<HTMLInputAttributes, "type"> = $props();
</script>

<input type="range" bind:value {...rest} />

<style>
	input {
		width: 100%;
		min-width: 0;
		height: var(--ctl-h);
		margin: 0;
		accent-color: var(--accent);
		cursor: pointer;
	}

	input:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
		border-radius: var(--radius-md);
	}
</style>
