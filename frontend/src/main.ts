// Bundled so every platform renders the same faces offline; see --font-ui and --font-mono in theme.css.
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./theme.css";
import { mount } from "svelte";
import App from "./App.svelte";

const app = mount(App, {
	target: document.getElementById("app")!,
});

export default app;
