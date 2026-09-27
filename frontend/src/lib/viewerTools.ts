import type { WindowMode } from '../api';

export type ActiveTool = 'pan' | 'scroll' | 'zoom' | 'window_level' | 'annotate_rect';

export type ImageOrientation = {
	flipH: boolean;
	flipV: boolean;
	rotation: 0 | 90 | 180 | 270;
};

export const DEFAULT_ORIENTATION: ImageOrientation = { flipH: false, flipV: false, rotation: 0 };

export const TOOL_ORDER: ActiveTool[] = ['pan', 'scroll', 'zoom', 'window_level', 'annotate_rect'];

export interface WlPreset {
	id: string;
	label: string;
	wc?: number;
	ww?: number;
	mode?: WindowMode;
}

export const WL_PRESETS: WlPreset[] = [
	{ id: 'default', label: 'Default', mode: 'default' },
	{ id: 'full_dynamic', label: 'Full Dynamic', mode: 'full_dynamic' },
	{ id: 'abdomen', label: 'CT Abdomen', wc: 60, ww: 400 },
	{ id: 'angio', label: 'CT Angio', wc: 300, ww: 600 },
	{ id: 'bone', label: 'CT Bone', wc: 300, ww: 1500 },
	{ id: 'brain', label: 'CT Brain', wc: 40, ww: 80 },
	{ id: 'chest', label: 'CT Chest', wc: -600, ww: 1500 },
	{ id: 'lung', label: 'CT Lung', wc: -600, ww: 1600 },
];

export const TOOL_LABELS: Record<ActiveTool, string> = {
	window_level: 'W/L',
	pan: 'Pan',
	zoom: 'Zoom',
	scroll: 'Scroll',
	annotate_rect: 'ROI',
};

export const TOOL_SHORTCUTS: Record<ActiveTool, string> = {
	window_level: 'W',
	pan: 'P',
	zoom: 'Z',
	scroll: 'S',
	annotate_rect: 'R',
};
