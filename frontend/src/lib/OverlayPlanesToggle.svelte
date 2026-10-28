<script lang="ts">
	import Button from "./ui/Button.svelte";

	/**
	 * Shows or hides the active file's own overlay planes (60xx). Like the
	 * References button, its toolbar slot is always laid out and only hidden
	 * while the file has no overlay planes, so stepping between files with
	 * and without them never moves the toolbar or the image.
	 */
	let {
		planes,
		shown = $bindable(),
	}: {
		/** How many overlay planes the active file declares. */
		planes: number;
		shown: boolean;
	} = $props();
</script>

<span class="overlay-planes-toggle" class:hidden={planes === 0} aria-hidden={planes === 0 ? "true" : undefined}>
	<Button
		icon="overlay-planes"
		onclick={() => shown = !shown}
		disabled={planes === 0}
		aria-pressed={shown}
		aria-label={`DICOM overlay${planes === 1 ? "" : "s"} ${shown ? "shown" : "hidden"}`}
		title={`${shown ? "Hide" : "Show"} the image's DICOM overlay${planes === 1 ? "" : "s"} (O)`}
	>
		Overlay{planes > 1 ? `s ${planes}` : ""}
		<span class="state" class:off={!shown} aria-hidden="true">{shown ? "On" : "Off"}</span>
	</Button>
</span>

<style>
	.overlay-planes-toggle {
		flex: none;
	}

	.overlay-planes-toggle.hidden {
		visibility: hidden;
	}

	.state {
		min-width: 3ch;
		color: var(--accent);
		font: 600 11px/14px var(--font-ui);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.state.off {
		color: var(--ink-muted);
	}
</style>
