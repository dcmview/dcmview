import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { execFileSync, spawn } from "node:child_process";
import { chromium } from "playwright";
import ffmpegPath from "ffmpeg-static";

function parseArguments(argv) {
	const values = {};
	for (let index = 0; index < argv.length; index += 2) {
		const key = argv[index];
		const value = argv[index + 1];
		if (!key?.startsWith("--") || value === undefined) {
			throw new Error(`invalid capture argument near ${key ?? "end of arguments"}`);
		}
		values[key.slice(2)] = value;
	}
	for (const required of ["url", "scene", "output", "report"]) {
		if (!values[required]) throw new Error(`missing --${required}`);
	}
	return values;
}

function instanceOrder(file) {
	const parsed = Number.parseInt(file.instance_number, 10);
	return Number.isFinite(parsed) ? parsed : Number.MAX_SAFE_INTEGER;
}

function chooseFile(files, scene) {
	let candidates = files
		.filter((file) => file.series_instance_uid === scene.series_instance_uid)
		.filter((file) => file.has_pixels)
		.sort((left, right) => instanceOrder(left) - instanceOrder(right) || left.path.localeCompare(right.path));
	if (scene.preferred_filename) {
		const preferred = candidates.find((file) => path.basename(file.path) === scene.preferred_filename);
		if (!preferred) throw new Error(`preferred file is unavailable: ${scene.preferred_filename}`);
		return preferred;
	}
	if (candidates.length === 0) {
		throw new Error(`no pixel-bearing file found for series ${scene.series_instance_uid}`);
	}
	const position = Math.min(1, Math.max(0, Number(scene.file_position ?? 0)));
	return candidates[Math.round(position * (candidates.length - 1))];
}

async function renderedToken(page) {
	return page.locator("canvas.dicom-canvas").getAttribute("data-capture-rendered");
}

async function waitForRendered(page, expectedFileIndex = null, expectedFrameIndex = null) {
	await page.waitForFunction(
		({ fileIndex, frameIndex }) => {
			const canvas = document.querySelector("canvas.dicom-canvas");
			if (!(canvas instanceof HTMLCanvasElement) || canvas.width <= 1 || canvas.height <= 1) return false;
			const token = canvas.dataset.captureRendered ?? "";
			if (!token) return false;
			if (fileIndex !== null && !token.startsWith(`${fileIndex}:`)) return false;
			if (frameIndex !== null && token !== `${fileIndex}:${frameIndex}`) return false;
			return !document.querySelector(".frame-request-indicator");
		},
		{ fileIndex: expectedFileIndex, frameIndex: expectedFrameIndex },
		{ timeout: 45_000 },
	);
	await page.evaluate(() => document.fonts.ready);
	await waitForTags(page);
}

async function waitForTags(page) {
	// Pixel rendering can finish before the tag tree (notably for WSI).
	await page.getByRole("button", { name: "Copy (0008,0016) SOPClassUID", exact: true })
		.waitFor({ state: "visible", timeout: 45_000 });
}

async function advanceFrame(page, step = 1) {
	for (let count = 0; count < step; count += 1) {
		const before = await renderedToken(page);
		const position = page.getByRole("slider", { name: "Image position" });
		await position.evaluate((input) => {
			if (!(input instanceof HTMLInputElement)) throw new Error("capture position is not an input");
			const maximum = Number(input.max);
			const current = Number(input.value);
			input.value = String(current >= maximum ? 0 : current + 1);
			input.dispatchEvent(new Event("input", { bubbles: true }));
		});
		await page.waitForFunction(
			(previous) => {
				const canvas = document.querySelector("canvas.dicom-canvas");
				return canvas instanceof HTMLCanvasElement
					&& Boolean(canvas.dataset.captureRendered)
					&& canvas.dataset.captureRendered !== previous
					&& !document.querySelector(".frame-request-indicator");
			},
			before,
			{ timeout: 45_000 },
		);
		await waitForTags(page);
	}
}

async function seekWithinFile(page, file, frameIndex) {
	if (!frameIndex) return;
	if (frameIndex >= file.frame_count) {
		throw new Error(`requested frame ${frameIndex} exceeds file frame count ${file.frame_count}`);
	}
	const position = page.getByRole("slider", { name: "Image position" });
	await position.evaluate((input, offset) => {
		if (!(input instanceof HTMLInputElement)) throw new Error("capture position is not an input");
		input.value = String(Number(input.value) + offset);
		input.dispatchEvent(new Event("input", { bubbles: true }));
	}, frameIndex);
	await waitForRendered(page, file.index, frameIndex);
}

async function captureGif(page, output, options) {
	if (!ffmpegPath) throw new Error("ffmpeg-static did not provide an executable path");
	const frames = Number(options.frames);
	const step = Number(options.step ?? 1);
	const fps = Number(options.fps);
	if (!Number.isInteger(frames) || frames < 2 || !Number.isInteger(step) || step < 1 || fps <= 0) {
		throw new Error("invalid GIF capture options");
	}
	const frameRoot = await mkdtemp(path.join(tmpdir(), "dcmview-marketing-frames-"));
	try {
		for (let index = 0; index < frames; index += 1) {
			if (index > 0) await advanceFrame(page, step);
			await page.screenshot({
				path: path.join(frameRoot, `frame-${String(index).padStart(4, "0")}.png`),
				animations: "disabled",
				caret: "hide",
			});
		}
		await new Promise((resolve, reject) => {
			const command = spawn(ffmpegPath, [
				"-hide_banner",
				"-loglevel", "error",
				"-y",
				"-framerate", String(fps),
				"-i", path.join(frameRoot, "frame-%04d.png"),
				"-filter_complex",
				"[0:v]split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle",
				"-loop", "0",
				output,
			], { stdio: ["ignore", "inherit", "inherit"] });
			command.once("error", reject);
			command.once("exit", (code) => code === 0 ? resolve() : reject(new Error(`ffmpeg exited ${code}`)));
		});
	} finally {
		await rm(frameRoot, { recursive: true, force: true });
	}
}

function digestText(value) {
	return createHash("sha256").update(value).digest("hex");
}

async function main() {
	const args = parseArguments(process.argv.slice(2));
	const scene = JSON.parse(await readFile(args.scene, "utf8"));
	await mkdir(path.dirname(args.output), { recursive: true });
	const browser = await chromium.launch({ headless: true });
	const context = await browser.newContext({
		viewport: { width: scene.viewport.width, height: scene.viewport.height },
		deviceScaleFactor: scene.viewport.device_scale_factor,
		colorScheme: scene.theme,
		locale: scene.locale,
		reducedMotion: "reduce",
	});
	const page = await context.newPage();
	try {
		await page.goto(args.url, { waitUntil: "domcontentloaded", timeout: 60_000 });
		await page.addStyleTag({ content: `
			*, *::before, *::after { animation: none !important; transition: none !important; caret-color: transparent !important; }
			.status { display: none !important; }
		` });
		const token = new URLSearchParams(new URL(args.url).hash.slice(1)).get("token");
		const catalog = await page.evaluate(async (token) => {
			const response = await fetch("/api/files", {
				headers: { Authorization: `Bearer ${token}` },
			});
			if (!response.ok) throw new Error(`files endpoint returned ${response.status}`);
			return response.json();
		}, token);
		const unexpectedPatientIds = [...new Set(catalog.files
			.map((file) => file.patient_id)
			.filter(Boolean)
			.filter((patientId) => !scene.allowed_patient_ids.includes(patientId)))];
		if (unexpectedPatientIds.length > 0) {
			throw new Error(`unexpected patient identifiers: ${unexpectedPatientIds.join(", ")}`);
		}
		const file = chooseFile(catalog.files, scene);
		const button = page.locator(`[data-capture-file-index="${file.index}"]`).first();
		await button.waitFor({ state: "visible", timeout: 30_000 });
		await button.click();
		await waitForRendered(page, file.index, 0);
		await seekWithinFile(page, file, Number(scene.frame ?? 0));
		if (scene.semantic_context) {
			const semantic = page.getByRole("button", { name: "Semantic Context", exact: true });
			await semantic.waitFor({ state: "visible", timeout: 30_000 });
			await semantic.click();
			await waitForRendered(page);
			if (scene.require_semantic_overlay) {
				const interpretation = await page.getByRole("region", { name: "Object interpretation" }).innerText();
				if (!interpretation.includes("Overlay eligible")) {
					throw new Error("capture scene requires an eligible semantic overlay");
				}
			}
		}

		const visibleText = await page.locator("body").innerText();
		for (const forbidden of ["/Users/", "\\Users\\", "marketing-source-data", "127.0.0.1", "localhost"]) {
			if (visibleText.includes(forbidden)) throw new Error(`visible text contains forbidden value: ${forbidden}`);
		}

		if (scene.kind === "gif") {
			await captureGif(page, args.output, scene.gif);
		} else {
			await page.screenshot({
				path: args.output,
				animations: "disabled",
				caret: "hide",
			});
		}
		const report = {
			scene_id: scene.id,
			file_index: file.index,
			sop_instance_uid: file.sop_instance_uid,
			series_instance_uid: file.series_instance_uid,
			patient_ids: [...new Set(catalog.files.map((candidate) => candidate.patient_id).filter(Boolean))].sort(),
			visible_text_sha256: digestText(visibleText),
			browser_version: browser.version(),
			node_version: process.version,
			ffmpeg_version: scene.kind === "gif"
				? execFileSync(ffmpegPath, ["-version"], { encoding: "utf8" }).split(/\r?\n/, 1)[0]
				: null,
			viewport: scene.viewport,
		};
		await import("node:fs/promises").then(({ writeFile }) =>
			writeFile(args.report, `${JSON.stringify(report, null, 2)}\n`, "utf8")
		);
	} finally {
		await browser.close();
	}
}

main().catch((error) => {
	console.error(error instanceof Error ? error.stack : String(error));
	process.exitCode = 1;
});
