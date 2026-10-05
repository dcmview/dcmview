<script lang="ts">
	import type { AccessDenial } from "../api";

	// The whole page when the server refuses the viewer's requests (401).
	let { denial }: { denial: AccessDenial } = $props();
</script>

<main class="access">
	{#if denial === "missing"}
		<h1>This viewer needs its access link</h1>
		<p>
			Copy the full URL, including <code>#token=…</code>, from the terminal where dcmview is running.
		</p>
	{:else}
		<h1>The viewer restarted</h1>
		<p>
			This page belongs to an earlier dcmview session. Reopen the viewer from the new link, including
			<code>#token=…</code>, in the terminal where dcmview is running.
		</p>
	{/if}
</main>

<style>
	.access {
		display: grid;
		place-content: center;
		justify-items: center;
		gap: 12px;
		height: 100vh;
		padding: 24px;
		background: var(--canvas);
		color: var(--text);
		text-align: center;
	}

	h1 {
		margin: 0;
		font-size: 1rem;
		font-weight: 600;
	}

	p {
		margin: 0;
		max-width: 46ch;
		color: var(--ink-muted);
	}

	code {
		font: var(--t-mono);
		color: var(--text);
	}
</style>
