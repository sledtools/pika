import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import { marmotPlugin } from "./src/channel.js";
import { marmotPluginConfigSchema } from "./src/config-schema.js";
import { setMarmotRuntime } from "./src/runtime.js";

const plugin = {
  id: "marmot",
  name: "Marmot",
  description: "Marmot MLS group messaging over Nostr (Rust sidecar)",
  configSchema: marmotPluginConfigSchema,
  register(api: OpenClawPluginApi) {
    setMarmotRuntime(api.runtime);
    api.registerChannel({ plugin: marmotPlugin });
  },
};

export default plugin;
