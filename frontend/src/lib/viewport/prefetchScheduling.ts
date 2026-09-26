export const PREFETCH_CONCURRENCY = 3;

type NetworkInformationLike = {
	saveData?: boolean;
	effectiveType?: string;
	addEventListener?: (type: string, listener: () => void) => void;
	removeEventListener?: (type: string, listener: () => void) => void;
};

function networkInformation(): NetworkInformationLike | undefined {
	return (navigator as { connection?: NetworkInformationLike }).connection;
}

/** Parallel prefetch requests suited to the browser's reported connection. */
export function prefetchConcurrencyFor(connection: NetworkInformationLike | undefined): number {
	if (!connection) return PREFETCH_CONCURRENCY;
	if (connection.saveData) return 1;
	const type = connection.effectiveType ?? "";
	if (type === "slow-2g" || type === "2g") return 1;
	if (type === "3g") return 2;
	return 4;
}

/**
 * Reports the prefetch concurrency now and whenever the connection changes.
 * Returns the unsubscribe function.
 */
export function observePrefetchConcurrency(update: (concurrency: number) => void): () => void {
	const connection = networkInformation();
	update(prefetchConcurrencyFor(connection));
	if (!connection?.addEventListener || !connection.removeEventListener) return () => {};
	const onChange = () => update(prefetchConcurrencyFor(connection));
	connection.addEventListener("change", onChange);
	return () => connection.removeEventListener?.("change", onChange);
}

/** Runs `fn` when the browser is idle (within `timeout` ms), or on the next task. */
export function scheduleIdle(fn: () => void, timeout = 200): void {
	if (typeof requestIdleCallback === "function") {
		requestIdleCallback(fn, { timeout });
	} else {
		setTimeout(fn, 0);
	}
}
