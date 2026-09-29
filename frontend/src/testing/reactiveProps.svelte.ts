/**
 * Independently reactive props for component transition tests. The testing
 * library's rerender invalidates every prop, which can rerun file-reset effects
 * even when only a frame or window changed. These cells behave like App's props.
 */
export function reactiveProps<Props extends object>(initial: Props) {
	const props = {} as Props;
	const setters = new Map<keyof Props, (value: Props[keyof Props]) => void>();
	for (const key of Object.keys(initial) as (keyof Props)[]) {
		let value = $state.raw(initial[key]);
		Object.defineProperty(props, key, { enumerable: true, configurable: true, get: () => value, set: (next) => { value = next; } });
		setters.set(key, (next) => { value = next; });
	}
	return {
		props,
		update(next: Partial<Props>) {
			for (const key of Object.keys(next) as (keyof Props)[]) setters.get(key)?.(next[key] as Props[keyof Props]);
		},
	};
}
