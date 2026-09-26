import { DEFAULT_ORIENTATION, type ImageOrientation } from "../viewerTools";
import { DEFAULT_TRANSFORM, sameTransform, type ViewTransform } from "./viewTransform";

/**
 * Zoom, pan, and orientation for each navigation scope (open tab). Keying by
 * scope keeps a stack of single-frame files in one view state, like frames
 * of one multiframe object, while a different tab starts from a fitted view.
 */
export class ViewStates {
	#transforms = $state<Record<string, ViewTransform>>({});
	#orientations = $state<Record<string, ImageOrientation>>({});

	/** The scope's stored transform, or undefined before its first fit or zoom. */
	storedTransform(scope: string): ViewTransform | undefined {
		return this.#transforms[scope];
	}

	transform(scope: string): ViewTransform {
		return (scope ? this.#transforms[scope] : undefined) ?? DEFAULT_TRANSFORM;
	}

	setTransform(scope: string, transform: Omit<ViewTransform, "fit">, fit = false): void {
		if (!scope) return;
		const next = { scale: transform.scale, tx: transform.tx, ty: transform.ty, fit };
		if (sameTransform(this.#transforms[scope], next)) return;
		this.#transforms = { ...this.#transforms, [scope]: next };
	}

	orientation(scope: string): ImageOrientation {
		return this.#orientations[scope] ?? DEFAULT_ORIENTATION;
	}

	updateOrientation(scope: string, change: (current: ImageOrientation) => ImageOrientation): void {
		if (!scope) return;
		this.#orientations = { ...this.#orientations, [scope]: change(this.orientation(scope)) };
	}

	resetOrientation(scope: string): void {
		if (!this.#orientations[scope]) return;
		this.#orientations = { ...this.#orientations, [scope]: DEFAULT_ORIENTATION };
	}
}
