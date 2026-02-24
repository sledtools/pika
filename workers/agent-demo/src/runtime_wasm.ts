import initWasm, { WasmRuntime } from "../vendor/pikachat-wasm/pikachat_wasm.js";
import type {
  CreateOutboundGroupMessageResult,
  InitOrLoadIdentityResult,
  KeyPackagePublishPayload,
  ProcessGroupMessageResult,
  ProcessWelcomeResult,
  RuntimeSnapshot,
} from "./runtime_contract";

let wasmInitPromise: Promise<void> | null = null;

function ensureWasmReady(): Promise<void> {
  if (!wasmInitPromise) {
    wasmInitPromise = (async () => {
      const moduleImport = await import("../vendor/pikachat-wasm/pikachat_wasm_bg.wasm");
      const moduleOrPath = (moduleImport as { default?: unknown }).default ?? moduleImport;
      await initWasm({ module_or_path: moduleOrPath as any });
    })();
  }
  return wasmInitPromise;
}

export class WasmRuntimeContractStateMachine {
  private runtime: WasmRuntime;

  private constructor(runtime: WasmRuntime) {
    this.runtime = runtime;
  }

  static async fromSnapshot(
    snapshot?: RuntimeSnapshot,
  ): Promise<WasmRuntimeContractStateMachine> {
    await ensureWasmReady();
    const runtime = new WasmRuntime();
    const machine = new WasmRuntimeContractStateMachine(runtime);
    if (snapshot) {
      machine.runtime.load_snapshot_json(JSON.stringify(snapshot));
    }
    return machine;
  }

  snapshot(): RuntimeSnapshot {
    return JSON.parse(this.runtime.snapshot_json()) as RuntimeSnapshot;
  }

  initOrLoadIdentity(secretSeedHint?: string): InitOrLoadIdentityResult {
    const out = this.runtime.init_or_load_identity_json(secretSeedHint ?? null);
    return JSON.parse(out) as InitOrLoadIdentityResult;
  }

  publishKeypackagePayload(): KeyPackagePublishPayload {
    const out = this.runtime.publish_keypackage_payload_json();
    return JSON.parse(out) as KeyPackagePublishPayload;
  }

  processWelcome(groupId: string): ProcessWelcomeResult {
    const out = this.runtime.process_welcome_json(groupId);
    return JSON.parse(out) as ProcessWelcomeResult;
  }

  processWelcomeEventJson(
    groupId: string,
    wrapperEventIdHex: string,
    welcomeEventJson: string,
  ): ProcessWelcomeResult {
    const out = this.runtime.process_welcome_event_json(
      groupId,
      wrapperEventIdHex,
      welcomeEventJson,
    );
    return JSON.parse(out) as ProcessWelcomeResult;
  }

  processGroupMessage(
    groupId: string,
    eventId: string,
    ciphertextB64: string,
  ): ProcessGroupMessageResult {
    const out = this.runtime.process_group_message_json(groupId, eventId, ciphertextB64);
    return JSON.parse(out) as ProcessGroupMessageResult;
  }

  processGroupMessageEventJson(groupId: string, eventJson: string): ProcessGroupMessageResult {
    const out = this.runtime.process_group_message_event_json(groupId, eventJson);
    return JSON.parse(out) as ProcessGroupMessageResult;
  }

  createOutboundGroupMessage(
    groupId: string,
    plaintext: string,
  ): CreateOutboundGroupMessageResult {
    const out = this.runtime.create_outbound_group_message_json(groupId, plaintext);
    return JSON.parse(out) as CreateOutboundGroupMessageResult;
  }
}
