/* tslint:disable */
/* eslint-disable */

export class WasmRuntime {
    free(): void;
    [Symbol.dispose](): void;
    create_outbound_group_message_json(group_id: string, plaintext: string): string;
    init_or_load_identity_json(secret_seed_hint?: string | null): string;
    load_snapshot_json(snapshot_json: string): void;
    constructor();
    process_group_message_event_json(group_id: string, event_json: string): string;
    process_group_message_json(group_id: string, event_id: string, ciphertext_b64: string): string;
    process_welcome_event_json(group_id: string, wrapper_event_id_hex: string, welcome_event_json: string): string;
    process_welcome_json(group_id: string): string;
    publish_keypackage_payload_json(): string;
    snapshot_json(): string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmruntime_free: (a: number, b: number) => void;
    readonly wasmruntime_create_outbound_group_message_json: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly wasmruntime_init_or_load_identity_json: (a: number, b: number, c: number) => [number, number, number, number];
    readonly wasmruntime_load_snapshot_json: (a: number, b: number, c: number) => [number, number];
    readonly wasmruntime_new: () => number;
    readonly wasmruntime_process_group_message_event_json: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly wasmruntime_process_group_message_json: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number, number];
    readonly wasmruntime_process_welcome_event_json: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number, number];
    readonly wasmruntime_process_welcome_json: (a: number, b: number, c: number) => [number, number, number, number];
    readonly wasmruntime_publish_keypackage_payload_json: (a: number) => [number, number, number, number];
    readonly wasmruntime_snapshot_json: (a: number) => [number, number, number, number];
    readonly rustsecp256k1_v0_10_0_context_create: (a: number) => number;
    readonly rustsecp256k1_v0_10_0_context_destroy: (a: number) => void;
    readonly rustsecp256k1_v0_10_0_default_error_callback_fn: (a: number, b: number) => void;
    readonly rustsecp256k1_v0_10_0_default_illegal_callback_fn: (a: number, b: number) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
