<script lang="ts">
	import type { CineDirection, CineMode } from "./cinePlayback";
	import Button from "./ui/Button.svelte";
	import ButtonGroup from "./ui/ButtonGroup.svelte";
	import SegmentedControl from "./ui/SegmentedControl.svelte";
	import Select from "./ui/Select.svelte";

	let {
		totalFrames,
		currentPosition,
		onpositionchange,
		cinePlaying = $bindable(),
		cineFps = $bindable(),
		cineMode = $bindable(),
		cineDirection = $bindable(),
	}: {
		totalFrames: number;
		currentPosition: number;
		onpositionchange: (position: number) => void;
		cinePlaying: boolean;
		cineFps: number;
		cineMode: CineMode;
		cineDirection: CineDirection;
	} = $props();

	const FPS_OPTIONS = [1, 5, 10, 15, 24];
	const MODE_OPTIONS: { value: CineMode; label: string; title: string }[] = [
		{ value: "loop", label: "Loop", title: "Restart from the first image" },
		{ value: "sweep", label: "Sweep", title: "Play forward, then back" },
	];

	/** Stops cine and moves one image back or forward, clamped to the stack. */
	export function step(delta: -1 | 1) {
		if (totalFrames <= 1) {
			return;
		}
		cinePlaying = false;
		onpositionchange(Math.max(0, Math.min(totalFrames - 1, currentPosition + delta)));
	}

	export function togglePlay() {
		if (totalFrames <= 1) return;
		if (!cinePlaying) {
			cineDirection = 1;
		}
		cinePlaying = !cinePlaying;
	}

	$effect(() => {
		if (currentPosition >= totalFrames && totalFrames > 0) {
			onpositionchange(0);
		}
		if (totalFrames <= 1) {
			cinePlaying = false;
		}
	});
</script>

{#if totalFrames > 1}
	<div class="slider">
		<input
			type="range"
			hidden
			data-capture-position
			min="0"
			max={Math.max(0, totalFrames - 1)}
			value={currentPosition}
			oninput={(event) => onpositionchange(Number(event.currentTarget.value))}
		/>
		<ButtonGroup label="Playback">
			<Button icon="prev" onclick={() => step(-1)} aria-label="Previous image" />
			<Button
				icon={cinePlaying ? "pause" : "play"}
				onclick={togglePlay}
				aria-label={cinePlaying ? "Pause cine" : "Play cine"}
			/>
			<Button icon="next" onclick={() => step(1)} aria-label="Next image" />
		</ButtonGroup>
		<span class="position">image {currentPosition + 1} / {totalFrames}</span>
		<Select aria-label="Cine speed" bind:value={cineFps}>
			{#each FPS_OPTIONS as f}
				<option value={f}>{f} fps</option>
			{/each}
		</Select>
		<SegmentedControl
			label="Cine mode"
			options={MODE_OPTIONS}
			value={cineMode}
			onchange={(mode) => { cineMode = mode; }}
		/>
	</div>
{/if}

<style>
	.slider {
		display: flex;
		flex-wrap: wrap;
		gap: 8px 10px;
		align-items: center;
		min-width: 0;
		min-height: var(--bar-h);
		padding: 6px 10px;
		box-sizing: border-box;
		background: var(--paper);
		border-top: 1px solid var(--line);
		color: var(--text);
	}

	.position {
		font: var(--t-mono);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
</style>
