<script lang="ts">
	import type { Snippet } from "svelte";
	import type { HTMLSelectAttributes } from "svelte/elements";
	import Icon from "./Icon.svelte";

	/** A native select drawn as a Bea secondary control, with its own chevron. */
	let { value = $bindable(), children, ...rest }: HTMLSelectAttributes & { children: Snippet } = $props();
</script>

<span class="select">
	<select bind:value {...rest}>
		{@render children()}
	</select>
	<Icon name="chevron-down" size={14} />
</span>

<style>
	.select {
		position: relative;
		display: inline-flex;
		align-items: center;
		flex: none;
		color: var(--ink-muted);
	}

	.select :global(.icon) {
		position: absolute;
		right: 8px;
		pointer-events: none;
	}

	select {
		appearance: none;
		height: var(--ctl-h);
		padding: 0 28px 0 10px;
		border: 1px solid var(--ink);
		border-radius: var(--radius-md);
		background: linear-gradient(var(--control-top), var(--control-bot));
		box-shadow: var(--elev-control-secondary);
		color: var(--text);
		font: 500 13px/18px var(--font-ui);
		cursor: pointer;
	}

	select:hover {
		background: var(--control-top);
	}

	select:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}
</style>
