<script lang="ts">
	import type { HTMLInputAttributes } from "svelte/elements";
	import Icon from "./Icon.svelte";

	/** A recessed mono input with a search glyph, for filters over identifiers. */
	let { value = $bindable(""), ...rest }: HTMLInputAttributes = $props();
</script>

<label class="field">
	<Icon name="search" size={14} />
	<input type="search" bind:value {...rest} />
</label>

<style>
	.field {
		display: flex;
		align-items: center;
		gap: 8px;
		height: var(--ctl-h);
		padding: 0 8px;
		box-sizing: border-box;
		min-width: 0;
		border: 1px solid var(--ink);
		border-radius: var(--radius-md);
		background: var(--field);
		box-shadow: var(--recess);
		color: var(--ink-muted);
	}

	.field:focus-within {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	input {
		flex: 1;
		min-width: 0;
		border: 0;
		outline: none;
		background: none;
		color: var(--text);
		font: var(--t-mono);
	}

	input::placeholder {
		color: var(--ink-muted);
	}

	input::-webkit-search-cancel-button {
		cursor: pointer;
	}
</style>
