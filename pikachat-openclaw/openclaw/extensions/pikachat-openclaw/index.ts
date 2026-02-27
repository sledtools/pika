import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import {
  pikachatPlugin,
  createSendHypernoteToolFactory,
  createSubmitHypernoteActionToolFactory,
} from "./src/channel.js";
import { pikachatPluginConfigSchema } from "./src/config-schema.js";
import { setPikachatRuntime } from "./src/runtime.js";

const plugin = {
  id: "pikachat-openclaw",
  name: "Pikachat",
  description: "Pikachat MLS group messaging over Nostr (Rust sidecar)",
  configSchema: pikachatPluginConfigSchema,
  register(api: OpenClawPluginApi) {
    setPikachatRuntime(api.runtime);
    api.registerChannel({ plugin: pikachatPlugin });
    api.registerTool(createSendHypernoteToolFactory() as any);
    api.registerTool(createSubmitHypernoteActionToolFactory() as any);
  },
};

export default plugin;
