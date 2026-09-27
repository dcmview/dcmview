<script lang="ts">
	import Button from "./ui/Button.svelte";
	import StatusBadge from "./ui/StatusBadge.svelte";

	let {
		serverStartMs,
		fileCount,
		reachable = true,
		onretry,
	}: {
		serverStartMs: number;
		fileCount: number;
		/** False once a request could not reach the server at all. */
		reachable?: boolean;
		onretry?: () => void;
	} = $props();

	let nowMs = $state(Date.now());
	$effect(() => {
		const timer = setInterval(() => {
			nowMs = Date.now();
		}, 1000);
		return () => clearInterval(timer);
	});

	const uptime = $derived.by(() => {
		const elapsedSeconds = Math.max(0, Math.floor((nowMs - serverStartMs) / 1000));
		const hours = String(Math.floor(elapsedSeconds / 3600)).padStart(2, "0");
		const minutes = String(Math.floor((elapsedSeconds % 3600) / 60)).padStart(2, "0");
		const seconds = String(elapsedSeconds % 60).padStart(2, "0");
		return `${hours}:${minutes}:${seconds}`;
	});
</script>

<footer class="status">
	<span>{window.location.origin}</span>
	<span>{fileCount} files loaded</span>
	{#if reachable}
		<span>uptime {uptime}</span>
	{:else}
		<span class="unreachable" role="alert">
			<StatusBadge status="negative">Disconnected</StatusBadge>
			<span>dcmview is not reachable: the viewer process may have stopped</span>
			<Button variant="ghost" onclick={onretry}>Retry</Button>
		</span>
	{/if}
</footer>

<style>
	.status {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		min-width: 0;
		height: 26px;
		padding: 0 12px;
		background: var(--surface);
		border-top: 1px solid var(--line);
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
		font-variant-numeric: tabular-nums;
	}

	.unreachable {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		color: var(--text);
		font-family: var(--font-ui);
	}

	.status span {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
