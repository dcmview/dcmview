import type { ImageDisplayGeometry } from "../imageGeometry";
import type { ImageOrientation } from "../viewerTools";

/**
 * Zoom and pan of the image layer inside the viewport, in CSS pixels. `fit`
 * marks a transform that follows the viewport size until the user zooms or
 * pans.
 */
export type ViewTransform = { scale: number; tx: number; ty: number; fit: boolean };

export type ClientPoint = { x: number; y: number };

/** Viewport client-space position of the image layer's untransformed origin. */
export type LayerOrigin = { left: number; top: number };

/** A client point pinned to the layer-space point under it for zooming. */
export type ZoomAnchor = {
	clientX: number;
	clientY: number;
	localX: number;
	localY: number;
};

export const MIN_ZOOM = 0.05;
export const MAX_ZOOM = 64;
export const ZOOM_STEPS = [0.05, 0.1, 0.2, 0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64];
export const DEFAULT_TRANSFORM: ViewTransform = { scale: 1, tx: 0, ty: 0, fit: false };

export function clampZoom(scale: number): number {
	return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, scale));
}

/** The next preset zoom level above or below `scale`, if any. */
export function nextZoomStep(scale: number, direction: 1 | -1): number | undefined {
	return direction > 0
		? ZOOM_STEPS.find((step) => step > scale + 0.001)
		: [...ZOOM_STEPS].reverse().find((step) => step < scale - 0.001);
}

export function sameTransform(a: ViewTransform | undefined, b: ViewTransform): boolean {
	return !!a
		&& a.fit === b.fit
		&& Math.abs(a.scale - b.scale) < 0.0001
		&& Math.abs(a.tx - b.tx) < 0.01
		&& Math.abs(a.ty - b.ty) < 0.01;
}

export function zoomAnchor(clientX: number, clientY: number, origin: LayerOrigin, transform: ViewTransform): ZoomAnchor {
	const { scale, tx, ty } = transform;
	return {
		clientX,
		clientY,
		localX: (clientX - origin.left - tx) / scale,
		localY: (clientY - origin.top - ty) / scale,
	};
}

/** Zooms to `scale` while keeping the anchored layer point under its client point. */
export function zoomAroundAnchor(
	scale: number,
	anchor: ZoomAnchor,
	origin: LayerOrigin,
): Omit<ViewTransform, "fit"> {
	const clamped = clampZoom(scale);
	return {
		scale: clamped,
		tx: anchor.clientX - origin.left - anchor.localX * clamped,
		ty: anchor.clientY - origin.top - anchor.localY * clamped,
	};
}

/** CSS transform for the image layer: zoom/pan, then orientation about the image center. */
export function layerTransformCss(
	transform: ViewTransform,
	orientation: ImageOrientation,
	geometry: ImageDisplayGeometry,
): string {
	const { tx, ty, scale } = transform;
	let css = `translate(${tx}px, ${ty}px) scale(${scale})`;
	const { flipH, flipV, rotation } = orientation;
	if (rotation !== 0 || flipH || flipV) {
		const cx = geometry.centerX;
		const cy = geometry.centerY;
		const sx = flipH ? -1 : 1;
		const sy = flipV ? -1 : 1;
		css += ` translate(${cx}px,${cy}px) rotate(${rotation}deg) scale(${sx},${sy}) translate(${-cx}px,${-cy}px)`;
	}
	return css;
}

/**
 * Maps a client point to continuous image coordinates (x = column, y = row,
 * pixel (c, r) spans [c, c + 1) x [r, r + 1)) by inverting
 * `layerTransformCss` and the pixel aspect ratio. Points outside the image
 * are returned unclamped; callers clamp or reject them.
 */
export function clientToImagePoint(
	client: ClientPoint,
	origin: LayerOrigin,
	transform: ViewTransform,
	orientation: ImageOrientation,
	geometry: ImageDisplayGeometry,
): ClientPoint {
	const { scale, tx, ty } = transform;
	const dx = (client.x - origin.left - tx) / scale - geometry.centerX;
	const dy = (client.y - origin.top - ty) / scale - geometry.centerY;
	const radians = (orientation.rotation * Math.PI) / 180;
	const cos = Math.cos(radians);
	const sin = Math.sin(radians);
	const unrotatedX = dx * cos + dy * sin;
	const unrotatedY = -dx * sin + dy * cos;
	const layerX = geometry.centerX + (orientation.flipH ? -unrotatedX : unrotatedX);
	const layerY = geometry.centerY + (orientation.flipV ? -unrotatedY : unrotatedY);
	return { x: layerX, y: layerY / geometry.pixelAspectRatio };
}

/**
 * Maps continuous image coordinates to viewport-local CSS pixels (relative to
 * the layer origin) by applying `layerTransformCss` and the pixel aspect
 * ratio; the inverse of `clientToImagePoint`.
 */
export function imageToViewportPoint(
	point: ClientPoint,
	transform: ViewTransform,
	orientation: ImageOrientation,
	geometry: ImageDisplayGeometry,
): ClientPoint {
	const { scale, tx, ty } = transform;
	const layerX = point.x - geometry.centerX;
	const layerY = point.y * geometry.pixelAspectRatio - geometry.centerY;
	const dx = orientation.flipH ? -layerX : layerX;
	const dy = orientation.flipV ? -layerY : layerY;
	const radians = (orientation.rotation * Math.PI) / 180;
	const cos = Math.cos(radians);
	const sin = Math.sin(radians);
	return {
		x: tx + (geometry.centerX + dx * cos - dy * sin) * scale,
		y: ty + (geometry.centerY + dx * sin + dy * cos) * scale,
	};
}

export function flipHorizontal(orientation: ImageOrientation): ImageOrientation {
	return { ...orientation, flipH: !orientation.flipH };
}

export function flipVertical(orientation: ImageOrientation): ImageOrientation {
	return { ...orientation, flipV: !orientation.flipV };
}

export function rotateClockwise(orientation: ImageOrientation): ImageOrientation {
	return { ...orientation, rotation: ((orientation.rotation + 90) % 360) as ImageOrientation["rotation"] };
}

export function rotateCounterClockwise(orientation: ImageOrientation): ImageOrientation {
	return { ...orientation, rotation: ((orientation.rotation + 270) % 360) as ImageOrientation["rotation"] };
}
