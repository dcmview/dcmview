/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";

export default defineConfig({
	// Relative asset URLs let a reverse proxy serve the viewer under a path prefix.
	base: "./",
	// svelteTesting resolves Svelte's browser build and cleans up rendered
	// components after each test; it only applies under Vitest.
	plugins: [svelte(), ...(process.env.VITEST ? [svelteTesting()] : [])],
	server: {
		proxy: {
			"/api": {
				target: "http://127.0.0.1:8888",
				changeOrigin: true,
			},
		},
	},
	build: {
		outDir: "dist",
		emptyOutDir: true,
	},
	test: {
		// Module tests run in Node; component tests opt into a DOM with a
		// `// @vitest-environment happy-dom` docblock.
		environment: "node",
	},
});
