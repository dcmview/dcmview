<script lang="ts">
	import { ICONS, type IconName } from "./icons";

	let { name, size = 16 }: { name: IconName; size?: number } = $props();
</script>

<svg class="icon" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" focusable="false">
	{@html ICONS[name]}
</svg>

<style>
	.icon {
		flex: none;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.6;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.icon :global(.icon-hand) {
		transform-origin: 8px 8px;
		animation: icon-spin 8s linear infinite;
	}

	@keyframes icon-spin {
		to { transform: rotate(360deg); }
	}
</style>
