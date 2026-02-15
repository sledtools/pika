import type { PluginRuntime } from "openclaw/plugin-sdk";

let runtime: PluginRuntime | null = null;

export function setMarmotRuntime(r: PluginRuntime): void {
  runtime = r;
}

export function getMarmotRuntime(): PluginRuntime {
  if (!runtime) {
    throw new Error("marmot runtime not set (plugin not registered?)");
  }
  return runtime;
}

