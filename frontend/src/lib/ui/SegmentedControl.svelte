<script lang="ts" generics="T extends string">
	import Icon from "./Icon.svelte";
	import type { IconName } from "./icons";

	type Option = { value: T; label: string; icon?: IconName; title?: string };

	/**
	 * A recessed track whose raised knob travels to the chosen option. The knob
	 * is measured from the pressed button, so options may differ in width.
	 */
	let {
		options,
		value,
		label,
		onchange,
		fill = false,
	}: { options: Option[]; value: T; label: string; onchange: (value: T) => void; fill?: boolean } = $props();

	let track: HTMLDivElement | undefined = $state();
	let knob = $state({ x: 0, width: 0 });

	function measure() {
		const pressed = track?.querySelector<HTMLButtonElement>('button[aria-pressed="true"]');
		knob = pressed ? { x: pressed.offsetLeft - 2, width: pressed.offsetWidth } : { x: 0, width: 0 };
	}

	$effect(() => {
		void value;
		void options;
		measure();
	});

	$effect(() => {
		if (!track || typeof ResizeObserver === "undefined") return;
		const observer = new ResizeObserver(measure);
		observer.observe(track);
		return () => observer.disconnect();
	});
</script>

<div class="seg" class:fill role="group" aria-label={label} bind:this={track}>
	<span
		class="knob"
		class:placed={knob.width > 0}
		style:width="{knob.width}px"
		style:transform="translateX({knob.x}px)"
	></span>
	{#each options as option (option.value)}
		<button
			type="button"
			aria-pressed={option.value === value}
			title={option.title}
			onclick={() => onchange(option.value)}
		>
			{#if option.icon}<Icon name={option.icon} />{/if}
			{option.label}
		</button>
	{/each}
</div>

<style>
	.seg {
		position: relative;
		display: inline-flex;
		flex: none;
		height: var(--ctl-h);
		padding: 2px;
		box-sizing: border-box;
		background: var(--track);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--recess);
	}

	.fill {
		display: flex;
		width: 100%;
	}

	.knob {
		position: absolute;
		top: 2px;
		bottom: 2px;
		left: 2px;
		box-sizing: border-box;
		background: var(--knob);
		border: 1px solid var(--ink);
		border-radius: var(--radius-sm);
		box-shadow: var(--elev-knob);
		opacity: 0;
		transition:
			transform var(--settle) var(--ease-spring-soft),
			width var(--settle) var(--ease-spring-soft);
	}

	.knob.placed {
		opacity: 1;
	}

	button {
		position: relative;
		z-index: 1;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		padding: 0 10px;
		border: 0;
		border-radius: var(--radius-sm);
		background: none;
		color: var(--ink-muted);
		font: 500 13px/18px var(--font-ui);
		white-space: nowrap;
		cursor: pointer;
		transition: color var(--quick) var(--ease-standard);
	}

	.fill button {
		flex: 1 1 0;
		font-size: 12px;
	}

	button[aria-pressed="true"] {
		color: var(--text);
	}

	button:hover {
		color: var(--text);
	}

	button:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}
</style>
