<script lang="ts">
	import type { Snippet } from "svelte";
	import type { HTMLButtonAttributes } from "svelte/elements";
	import Icon from "./Icon.svelte";
	import type { IconName } from "./icons";

	/**
	 * Bea's secondary control at ctl-h: a faint gradient, ink edge and contact
	 * shadow that sinks 1px when pressed. `ghost` drops the edge and fill until
	 * hover, for low-emphasis actions. Icon-only buttons need an aria-label.
	 */
	let {
		variant = "secondary",
		icon,
		children,
		type = "button",
		element = $bindable(),
		...rest
	}: HTMLButtonAttributes & {
		variant?: "secondary" | "ghost";
		icon?: IconName;
		children?: Snippet;
		element?: HTMLButtonElement | null;
	} = $props();
</script>

<button bind:this={element} {type} class="btn {variant}" class:icon-only={!children} {...rest}>
	{#if icon}<Icon name={icon} />{/if}
	{@render children?.()}
</button>

<style>
	.btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: var(--ctl-h);
		padding: 0 10px;
		border: 1px solid var(--ink);
		border-radius: var(--radius-md);
		background: linear-gradient(var(--control-top), var(--control-bot));
		box-shadow: var(--elev-control-secondary);
		color: var(--text);
		font: 500 13px/18px var(--font-ui);
		white-space: nowrap;
		cursor: pointer;
		transition:
			transform var(--instant) var(--ease-standard),
			box-shadow var(--instant) var(--ease-standard);
	}

	.btn:hover:not(:disabled) {
		background: var(--control-top);
	}

	.btn:active:not(:disabled) {
		transform: translate(1px, 1px);
		box-shadow: var(--press);
	}

	.btn:disabled {
		color: var(--subtle);
		cursor: default;
	}

	.btn:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	.icon-only {
		width: var(--ctl-h);
		padding: 0;
	}

	.ghost {
		border-color: transparent;
		background: none;
		box-shadow: none;
		color: var(--ink-muted);
	}

	.ghost:hover:not(:disabled) {
		background: var(--row-hover);
		color: var(--text);
	}

	.ghost:active:not(:disabled) {
		box-shadow: none;
	}
</style>
