import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { chromium } from "playwright";
import ffmpegPath from "ffmpeg-static";
import vscodeTest from "@vscode/test-electron";

const { downloadAndUnzipVSCode } = vscodeTest;

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
	for (const required of ["source", "scene", "output", "report", "binary", "repo"]) {
		if (!values[required]) throw new Error(`missing --${required}`);
	}
	return values;
}

function instanceOrder(file) {
	const parsed = Number.parseInt(file.instance_number, 10);
	return Number.isFinite(parsed) ? parsed : Number.MAX_SAFE_INTEGER;
}

function chooseFile(files, scene) {
	const candidates = files
		.filter((file) => file.series_instance_uid === scene.series_instance_uid && file.has_pixels)
		.sort((left, right) => instanceOrder(left) - instanceOrder(right) || left.path.localeCompare(right.path));
	if (candidates.length === 0) throw new Error(`series is unavailable: ${scene.series_instance_uid}`);
	const position = Math.min(1, Math.max(0, Number(scene.file_position ?? 0)));
	return candidates[Math.round(position * (candidates.length - 1))];
}

async function waitForViewerFrame(window) {
	const deadline = Date.now() + 90_000;
	while (Date.now() < deadline) {
		const frame = window.frames().find((candidate) => /^http:\/\/(127\.0\.0\.1|localhost):\d+\/(?:\?[^#]*)?$/.test(candidate.url()));
		if (frame) return frame;
		await window.waitForTimeout(250);
	}
	throw new Error(`VS Code webview did not load the dcmview frame: ${window.frames().map(frame => frame.url()).join(", ")}`);
}

async function waitForRendered(frame, fileIndex = null) {
	await frame.waitForFunction(
		(expected) => {
			const canvas = document.querySelector("canvas.dicom-canvas");
			return canvas instanceof HTMLCanvasElement
				&& canvas.width > 1
				&& canvas.height > 1
				&& canvas.dataset.captureRendered?.startsWith(`${expected}:`)
				&& !document.querySelector(".frame-request-indicator");
		},
		fileIndex,
		{ timeout: 60_000 },
	);
	await frame.evaluate(() => document.fonts.ready);
}

async function advanceViewerFrame(frame) {
	const canvas = frame.locator("canvas.dicom-canvas");
	const before = await canvas.getAttribute("data-capture-rendered");
	const position = frame.getByRole("slider", { name: "Image position" });
	await position.evaluate((input) => {
		if (!(input instanceof HTMLInputElement)) throw new Error("capture position is not an input");
		const maximum = Number(input.max);
		const current = Number(input.value);
		input.value = String(current >= maximum ? 0 : current + 1);
		input.dispatchEvent(new Event("input", { bubbles: true }));
	});
	await frame.waitForFunction(
		(previous) => {
			const rendered = document.querySelector("canvas.dicom-canvas")?.getAttribute("data-capture-rendered");
			return Boolean(rendered) && rendered !== previous && !document.querySelector(".frame-request-indicator");
		},
		before,
		{ timeout: 60_000 },
	);
}

async function availablePort() {
	return new Promise((resolve, reject) => {
		const server = net.createServer();
		server.once("error", reject);
		server.listen(0, "127.0.0.1", () => {
			const address = server.address();
			if (!address || typeof address === "string") {
				server.close(() => reject(new Error("failed to reserve a CDP port")));
				return;
			}
			server.close((error) => error ? reject(error) : resolve(address.port));
		});
	});
}

async function connectToCode(port, child) {
	const deadline = Date.now() + 60_000;
	let lastError;
	while (Date.now() < deadline) {
		if (child.exitCode !== null) throw new Error(`VS Code exited before CDP startup (${child.exitCode})`);
		try {
			return await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
		} catch (error) {
			lastError = error;
			await new Promise((resolve) => setTimeout(resolve, 250));
		}
	}
	throw new Error(`timed out connecting to VS Code CDP: ${lastError instanceof Error ? lastError.message : lastError}`);
}

async function stopProcess(child) {
	if (child.exitCode !== null) return;
	child.kill("SIGTERM");
	await Promise.race([
		new Promise((resolve) => child.once("exit", resolve)),
		new Promise((resolve) => setTimeout(resolve, 5_000)),
	]);
	if (child.exitCode === null) child.kill("SIGKILL");
}

async function findSeriesDirectory(root, seriesInstanceUid) {
	const pending = [root];
	while (pending.length > 0) {
		const directory = pending.shift();
		const basename = path.basename(directory);
		if (basename === seriesInstanceUid || basename.endsWith(`_${seriesInstanceUid}`)) return directory;
		for (const entry of await readdir(directory, { withFileTypes: true })) {
			if (entry.isDirectory()) pending.push(path.join(directory, entry.name));
		}
	}
	throw new Error(`series directory is unavailable: ${seriesInstanceUid}`);
}

async function setCaption(window, text) {
	await window.evaluate((caption) => {
		let element = document.querySelector("[data-dcmview-capture-caption]");
		if (!(element instanceof HTMLDivElement)) {
			element = document.createElement("div");
			element.dataset.dcmviewCaptureCaption = "true";
			Object.assign(element.style, {
				position: "fixed",
				left: "50%",
				bottom: "34px",
				transform: "translateX(-50%)",
				zIndex: "100000",
				padding: "10px 18px",
				borderRadius: "8px",
				background: "rgba(8, 12, 20, 0.92)",
				border: "1px solid rgba(255, 255, 255, 0.22)",
				boxShadow: "0 8px 28px rgba(0, 0, 0, 0.45)",
				color: "#f8fafc",
				font: "600 16px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
				letterSpacing: "0.01em",
				pointerEvents: "none",
			});
			document.body.append(element);
		}
		element.textContent = caption;
	}, text);
}

async function appendFrames(window, frameRoot, start, count) {
	for (let offset = 0; offset < count; offset += 1) {
		await window.screenshot({
			path: path.join(frameRoot, `frame-${String(start + offset).padStart(4, "0")}.png`),
			animations: "disabled",
			caret: "hide",
		});
	}
	return start + count;
}

async function encodeGif(frameRoot, output, fps) {
	if (!ffmpegPath) throw new Error("ffmpeg-static did not provide an executable path");
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
}

async function setEditorTheme(settingsPath, theme) {
	await writeFile(settingsPath, `${JSON.stringify({
		"window.menuStyle": "custom",
		"workbench.colorTheme": theme === "dark" ? "Default Dark Modern" : "Default Light Modern",
	}, null, 2)}\n`, "utf8");
}

async function waitForEditorTheme(window, theme) {
	await window.waitForFunction((expected) =>
		document.querySelector(".monaco-workbench")?.classList.contains(expected === "dark" ? "vs-dark" : "vs"),
		theme, { timeout: 30_000 });
}

async function assertViewerFollowsEditor(window, frame, settingsPath, initialTheme) {
	// This is the real Electron webview, including its initial URL and the
	// extension's live theme messages. Do not override the viewer's DOM/theme.
	assert.equal(new URL(frame.url()).searchParams.get("theme"), initialTheme);
	const checks = [];
	for (const [index, theme] of [initialTheme, initialTheme === "dark" ? "light" : "dark", initialTheme].entries()) {
		if (index > 0) await setEditorTheme(settingsPath, theme);
		await waitForEditorTheme(window, theme);
		await frame.waitForFunction((expected) => document.documentElement.dataset.theme === expected,
			theme, { timeout: 30_000 });
		checks.push(`${index === 0 ? "initial" : "switch"}-${theme}`);
	}
	return checks;
}

async function main() {
	const args = parseArguments(process.argv.slice(2));
	const scene = JSON.parse(await readFile(args.scene, "utf8"));
	const vscodeVersion = scene.vscode_version;
	if (typeof vscodeVersion !== "string" || !vscodeVersion) {
		throw new Error("VS Code capture scene must pin vscode_version");
	}
	await mkdir(path.dirname(args.output), { recursive: true });
	const executablePath = await downloadAndUnzipVSCode(vscodeVersion);
	const scratch = await mkdtemp(path.join(tmpdir(), "dcmview-vscode-capture-"));
	const workspaceRoot = path.join(scratch, "workspace");
	await mkdir(workspaceRoot);
	const seriesDirectory = await findSeriesDirectory(args.source, scene.series_instance_uid);
	const workspaceSeriesName = "Chest CT series";
	await symlink(seriesDirectory, path.join(workspaceRoot, workspaceSeriesName), "dir");
	const userDataDirectory = path.join(scratch, "user-data");
	await mkdir(path.join(userDataDirectory, "User"), { recursive: true });
	const settingsPath = path.join(userDataDirectory, "User", "settings.json");
	assert.ok(scene.theme === "dark" || scene.theme === "light");
	await setEditorTheme(settingsPath, scene.theme);
	const cdpPort = await availablePort();
	const code = spawn(executablePath, [
			workspaceRoot,
			"--new-window",
			`--remote-debugging-port=${cdpPort}`,
			`--extensionDevelopmentPath=${path.join(args.repo, "vscode")}`,
			`--user-data-dir=${userDataDirectory}`,
			`--extensions-dir=${path.join(scratch, "extensions")}`,
			"--disable-workspace-trust",
			"--disable-updates",
			"--disable-extension", "vscode.git",
			"--skip-release-notes",
			"--skip-welcome",
			"--window-size=1440,900",
		], {
		env: {
			...process.env,
			DCMVIEW_BINARY: args.binary,
			DCMVIEW_VSCODE_BYPASS: "0",
		},
		stdio: "ignore",
	});
	let browser;
	let frameRoot;
	try {
		browser = await connectToCode(cdpPort, code);
		const context = browser.contexts()[0];
		if (!context) throw new Error("VS Code CDP exposed no browser context");
		const window = context.pages()[0] ?? await context.waitForEvent("page", { timeout: 30_000 });
		await window.setViewportSize({ width: scene.viewport.width, height: scene.viewport.height });
		await window.waitForTimeout(2_000);
		await waitForEditorTheme(window, scene.theme);
		await window.addStyleTag({ content: ".monaco-hover { display: none !important; }" });
		const walkthrough = scene.walkthrough;
		if (scene.kind !== "gif" || typeof walkthrough !== "object" || walkthrough === null) {
			throw new Error("VS Code workflow capture requires GIF walkthrough settings");
		}
		const fps = Number(walkthrough.fps);
		const captionFrames = Number(walkthrough.caption_frames);
		const actionFrames = Number(walkthrough.action_frames);
		const viewerFrames = Number(walkthrough.viewer_frames);
		if (![fps, captionFrames, actionFrames, viewerFrames].every(Number.isInteger)
			|| [fps, captionFrames, actionFrames, viewerFrames].some((value) => value <= 0)) {
			throw new Error("invalid VS Code walkthrough timing");
		}
		frameRoot = await mkdtemp(path.join(tmpdir(), "dcmview-vscode-frames-"));
		let frameNumber = 0;
		const explorerItem = window.locator(".monaco-list-row").filter({ hasText: workspaceSeriesName }).first();
		await explorerItem.waitFor({ state: "visible", timeout: 30_000 });
		await explorerItem.click();
		await setCaption(window, "Right-click a DICOM file or folder");
		frameNumber = await appendFrames(window, frameRoot, frameNumber, captionFrames);
		await explorerItem.click({ button: "right" });
		await window.waitForTimeout(500);
		const menuLabels = await window.locator(".monaco-menu .action-label").allInnerTexts();
		if (menuLabels.length === 0) {
			throw new Error("VS Code Explorer context menu did not open");
		}
		const openAction = window.locator(".monaco-menu .action-label").filter({ hasText: /Open with dcmview/ }).first();
		await openAction.waitFor({ state: "visible", timeout: 30_000 });
		await setCaption(window, "Choose Open with dcmview");
		frameNumber = await appendFrames(window, frameRoot, frameNumber, actionFrames);
		await openAction.click();
		await window.mouse.move(1200, 40);
		const frame = await waitForViewerFrame(window);
		const themeChecks = await assertViewerFollowsEditor(window, frame, settingsPath, scene.theme);
		await frame.addStyleTag({ content: `
			*, *::before, *::after { animation: none !important; transition: none !important; caret-color: transparent !important; }
			.status { display: none !important; }
		` });
		const catalog = await frame.evaluate(async () => {
			const response = await fetch("/api/files");
			if (!response.ok) throw new Error(`files endpoint returned ${response.status}`);
			return response.json();
		});
		const unexpectedPatientIds = [...new Set(catalog.files
			.map((file) => file.patient_id)
			.filter(Boolean)
			.filter((patientId) => !scene.allowed_patient_ids.includes(patientId)))];
		if (unexpectedPatientIds.length > 0) {
			throw new Error(`unexpected patient identifiers: ${unexpectedPatientIds.join(", ")}`);
		}
		const file = chooseFile(catalog.files, scene);
		const button = frame.locator(`[data-capture-file-index="${file.index}"]`).first();
		await button.waitFor({ state: "visible", timeout: 30_000 });
		await button.click();
		await waitForRendered(frame, file.index);
		await setCaption(window, "Inspect and cine through the study inside VS Code");
		for (let index = 0; index < viewerFrames; index += 1) {
			if (index > 0 && index % 2 === 0) {
				await advanceViewerFrame(frame);
			}
			frameNumber = await appendFrames(window, frameRoot, frameNumber, 1);
		}
		const visibleText = await window.locator("body").innerText();
		for (const forbidden of [
			"/Users/", "\\Users\\", "/private/", "/var/folders/",
			"marketing-source-data", "dcmview-vscode-capture-", "127.0.0.1", "localhost",
		]) {
			if (visibleText.includes(forbidden)) throw new Error(`visible VS Code text contains forbidden value: ${forbidden}`);
		}
		await encodeGif(frameRoot, args.output, fps);
		await writeFile(args.report, `${JSON.stringify({
			scene_id: scene.id,
			file_index: file.index,
			sop_instance_uid: file.sop_instance_uid,
			series_instance_uid: file.series_instance_uid,
			patient_ids: [...new Set(catalog.files.map((candidate) => candidate.patient_id).filter(Boolean))].sort(),
			visible_text_sha256: createHash("sha256").update(visibleText).digest("hex"),
			vscode_version: vscodeVersion,
			theme_checks: themeChecks,
			node_version: process.version,
			ffmpeg_version: execFileSync(ffmpegPath, ["-version"], { encoding: "utf8" }).split(/\r?\n/, 1)[0],
			captured_frames: frameNumber,
			viewport: scene.viewport,
		}, null, 2)}\n`, "utf8");
	} finally {
		await stopProcess(code);
		if (browser) {
			await Promise.race([
				browser.close().catch(() => {}),
				new Promise((resolve) => setTimeout(resolve, 2_000)),
			]);
		}
		if (frameRoot) await rm(frameRoot, { recursive: true, force: true });
		await rm(scratch, { recursive: true, force: true });
	}
}

main().catch((error) => {
	console.error(error instanceof Error ? error.stack : String(error));
	process.exitCode = 1;
});
