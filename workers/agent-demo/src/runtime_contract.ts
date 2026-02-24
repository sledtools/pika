export const RUNTIME_SNAPSHOT_SCHEMA_VERSION = 2;

type RuntimeIdentityStateV1 = {
  pubkey_hint: string;
  key_package_hint: string;
};

export type RuntimeIdentityState = {
  pubkey_hex: string;
  secret_key_hex: string;
  key_package_hint: string;
};

export type RuntimeGroupState = {
  group_id: string;
  mls_group_id_hex: string;
  nostr_group_id_hex: string;
  processed_welcomes: number;
  applied_messages: number;
  outbound_messages: number;
};

export type RuntimeSnapshot = {
  schema_version: number;
  identity: RuntimeIdentityState | null;
  groups: Record<string, RuntimeGroupState>;
  outbound_counter: number;
  mdk_storage_snapshot_b64: string | null;
};

type RuntimeSnapshotV1 = {
  schema_version: number;
  identity: RuntimeIdentityStateV1 | null;
  groups: Record<string, RuntimeGroupState>;
  outbound_counter: number;
};

export type InitOrLoadIdentityResult = {
  created: boolean;
  pubkey_hint: string;
  key_package_hint: string;
};

export type KeyPackagePublishPayload = {
  pubkey_hint: string;
  key_package_hint: string;
  payload_json: string;
  tags_json: string;
  event_json: string;
  event_id: string;
};

export type ProcessWelcomeResult = {
  group_id: string;
  created_group: boolean;
  processed_welcomes: number;
  mls_group_id_hex: string;
  nostr_group_id_hex: string;
};

export type ProcessGroupMessageResult = {
  group_id: string;
  event_id: string;
  applied_messages: number;
  message_kind:
    | "application"
    | "proposal"
    | "ignored_proposal"
    | "external_join_proposal"
    | "commit"
    | "unprocessable"
    | "previously_failed"
    | "unknown";
  plaintext: string | null;
};

export type CreateOutboundGroupMessageResult = {
  group_id: string;
  event_id: string;
  sequence: number;
  ciphertext_b64: string;
  event_json: string;
};

function toHexHint(input: string): string {
  const normalized = input.trim().toLowerCase() || "seed";
  const hexAlphabet = "0123456789abcdef";
  let out = "";
  for (let i = 0; i < normalized.length && out.length < 24; i += 1) {
    const code = normalized.charCodeAt(i);
    out += hexAlphabet[code % 16];
    out += hexAlphabet[(code >> 4) % 16];
  }
  if (out.length < 24) {
    out = `${out}${"0".repeat(24)}`;
  }
  return out.slice(0, 24);
}

function toHex64(input: string): string {
  const base = toHexHint(input);
  const repeated = base.repeat(Math.ceil(64 / Math.max(base.length, 1)));
  return repeated.slice(0, 64);
}

function toBase64Utf8(input: string): string {
  const bytes = new TextEncoder().encode(input);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function normalizeGroupId(groupId: string): string {
  return groupId.trim();
}

function normalizeGroupState(groupId: string, input?: Partial<RuntimeGroupState>): RuntimeGroupState {
  return {
    group_id: groupId,
    mls_group_id_hex: String(input?.mls_group_id_hex ?? "").trim(),
    nostr_group_id_hex: String(input?.nostr_group_id_hex ?? "").trim(),
    processed_welcomes: Number(input?.processed_welcomes ?? 0),
    applied_messages: Number(input?.applied_messages ?? 0),
    outbound_messages: Number(input?.outbound_messages ?? 0),
  };
}

export function defaultRuntimeSnapshot(): RuntimeSnapshot {
  return {
    schema_version: RUNTIME_SNAPSHOT_SCHEMA_VERSION,
    identity: null,
    groups: {},
    outbound_counter: 0,
    mdk_storage_snapshot_b64: null,
  };
}

function normalizeRuntimeSnapshotV1(snapshot: RuntimeSnapshotV1): RuntimeSnapshot {
  const groups: Record<string, RuntimeGroupState> = {};
  for (const [groupId, groupState] of Object.entries(snapshot.groups ?? {})) {
    const normalizedGroupId = groupId.trim();
    if (!normalizedGroupId) {
      continue;
    }
    groups[normalizedGroupId] = normalizeGroupState(normalizedGroupId, groupState);
  }

  return {
    schema_version: RUNTIME_SNAPSHOT_SCHEMA_VERSION,
    identity: null,
    groups,
    outbound_counter: Number(snapshot.outbound_counter ?? 0),
    mdk_storage_snapshot_b64: null,
  };
}

export function normalizeRuntimeSnapshot(
  snapshot?: RuntimeSnapshot | RuntimeSnapshotV1,
): RuntimeSnapshot {
  if (!snapshot) {
    return defaultRuntimeSnapshot();
  }
  if (snapshot.schema_version === RUNTIME_SNAPSHOT_SCHEMA_VERSION) {
    const snapshotV2 = snapshot as RuntimeSnapshot;
    const groups: Record<string, RuntimeGroupState> = {};
    for (const [groupId, groupState] of Object.entries(snapshotV2.groups ?? {})) {
      const normalizedGroupId = groupId.trim();
      if (!normalizedGroupId) {
        continue;
      }
      groups[normalizedGroupId] = normalizeGroupState(normalizedGroupId, groupState);
    }

    return {
      schema_version: RUNTIME_SNAPSHOT_SCHEMA_VERSION,
      identity: snapshotV2.identity
        ? {
            pubkey_hex: String(snapshotV2.identity.pubkey_hex ?? "").trim(),
            secret_key_hex: String(snapshotV2.identity.secret_key_hex ?? "").trim(),
            key_package_hint: String(snapshotV2.identity.key_package_hint ?? "").trim(),
          }
        : null,
      groups,
      outbound_counter: Number(snapshotV2.outbound_counter ?? 0),
      mdk_storage_snapshot_b64: snapshotV2.mdk_storage_snapshot_b64 ?? null,
    };
  }
  if (snapshot.schema_version === 1) {
    return normalizeRuntimeSnapshotV1(snapshot as RuntimeSnapshotV1);
  }
  throw new Error(
    `snapshot schema mismatch: expected ${RUNTIME_SNAPSHOT_SCHEMA_VERSION}, got ${snapshot.schema_version}`,
  );
}

export class RuntimeContractStateMachine {
  private state: RuntimeSnapshot;

  constructor(snapshot?: RuntimeSnapshot) {
    this.state = structuredClone(normalizeRuntimeSnapshot(snapshot));
  }

  snapshot(): RuntimeSnapshot {
    return structuredClone(this.state);
  }

  initOrLoadIdentity(secretSeedHint?: string): InitOrLoadIdentityResult {
    if (this.state.identity) {
      return {
        created: false,
        pubkey_hint: this.state.identity.pubkey_hex,
        key_package_hint: this.state.identity.key_package_hint,
      };
    }

    const seed = secretSeedHint ?? "generated-seed";
    const hint = toHexHint(seed);
    this.state.identity = {
      pubkey_hex: toHex64(`pk:${seed}`),
      secret_key_hex: toHex64(`sk:${seed}`),
      key_package_hint: `kp_${hint}`,
    };
    return {
      created: true,
      pubkey_hint: this.state.identity.pubkey_hex,
      key_package_hint: this.state.identity.key_package_hint,
    };
  }

  publishKeypackagePayload(): KeyPackagePublishPayload {
    const identity = this.requireIdentity();
    const event_id = `kp_${identity.pubkey_hex}`;
    const tags_json = JSON.stringify([]);
    const payload_json = JSON.stringify({
      schema_version: RUNTIME_SNAPSHOT_SCHEMA_VERSION,
      pubkey_hex: identity.pubkey_hex,
      key_package_hint: identity.key_package_hint,
      kind: 443,
    });
    const event_json = JSON.stringify({
      id: event_id,
      pubkey: identity.pubkey_hex,
      sig: "stub-signature",
      kind: 443,
      created_at: Math.floor(Date.now() / 1000),
      tags: [],
      content: payload_json,
    });
    return {
      pubkey_hint: identity.pubkey_hex,
      key_package_hint: identity.key_package_hint,
      payload_json,
      tags_json,
      event_json,
      event_id,
    };
  }

  processWelcome(groupId: string): ProcessWelcomeResult {
    this.requireIdentity();
    const normalized = normalizeGroupId(groupId);
    if (!normalized) {
      throw new Error("group_id is required");
    }

    const createdGroup = !this.state.groups[normalized];
    const group = this.group(normalized);
    group.processed_welcomes += 1;
    if (!group.mls_group_id_hex) {
      group.mls_group_id_hex = toHex64(`mls:${normalized}`);
    }
    if (!group.nostr_group_id_hex) {
      group.nostr_group_id_hex = toHex64(`nostr:${normalized}`);
    }

    return {
      group_id: normalized,
      created_group: createdGroup,
      processed_welcomes: group.processed_welcomes,
      mls_group_id_hex: group.mls_group_id_hex,
      nostr_group_id_hex: group.nostr_group_id_hex,
    };
  }

  processWelcomeEventJson(
    groupId: string,
    _wrapperEventIdHex: string,
    _welcomeEventJson: string,
  ): ProcessWelcomeResult {
    return this.processWelcome(groupId);
  }

  processGroupMessage(
    groupId: string,
    eventId: string,
    ciphertextB64: string,
  ): ProcessGroupMessageResult {
    this.requireIdentity();
    const normalizedGroup = normalizeGroupId(groupId);
    if (!normalizedGroup) {
      throw new Error("group_id is required");
    }
    const normalizedEvent = eventId.trim();
    if (!normalizedEvent) {
      throw new Error("event_id is required");
    }
    if (!ciphertextB64.trim()) {
      throw new Error("ciphertext_b64 is required");
    }

    const group = this.group(normalizedGroup);
    group.applied_messages += 1;
    const plaintext = decodeScaffoldCiphertext(ciphertextB64);
    return {
      group_id: normalizedGroup,
      event_id: normalizedEvent,
      applied_messages: group.applied_messages,
      message_kind: plaintext ? "application" : "unknown",
      plaintext,
    };
  }

  processGroupMessageEventJson(groupId: string, eventJson: string): ProcessGroupMessageResult {
    const parsed = JSON.parse(eventJson) as {
      id?: unknown;
      content?: unknown;
    };
    return this.processGroupMessage(
      groupId,
      String(parsed.id ?? "").trim(),
      String(parsed.content ?? ""),
    );
  }

  createOutboundGroupMessage(
    groupId: string,
    plaintext: string,
  ): CreateOutboundGroupMessageResult {
    const identity = this.requireIdentity();
    const normalizedGroup = normalizeGroupId(groupId);
    if (!normalizedGroup) {
      throw new Error("group_id is required");
    }
    const normalizedContent = plaintext.trim();
    if (!normalizedContent) {
      throw new Error("plaintext is required");
    }

    const group = this.group(normalizedGroup);
    if (!group.nostr_group_id_hex) {
      group.nostr_group_id_hex = toHex64(`nostr:${normalizedGroup}`);
    }

    this.state.outbound_counter += 1;
    const sequence = this.state.outbound_counter;
    const eventId = `evt_${normalizedGroup}_${sequence}`;
    const payload = JSON.stringify({
      schema_version: RUNTIME_SNAPSHOT_SCHEMA_VERSION,
      group_id: normalizedGroup,
      sequence,
      from: identity.pubkey_hex,
      content: normalizedContent,
    });

    group.outbound_messages += 1;

    const ciphertext_b64 = toBase64Utf8(payload);
    return {
      group_id: normalizedGroup,
      event_id: eventId,
      sequence,
      ciphertext_b64,
      event_json: JSON.stringify({
        id: eventId,
        pubkey: identity.pubkey_hex,
        sig: "stub-signature",
        kind: 445,
        created_at: Math.floor(Date.now() / 1000),
        tags: [["h", group.nostr_group_id_hex]],
        content: ciphertext_b64,
      }),
    };
  }

  private requireIdentity(): RuntimeIdentityState {
    if (!this.state.identity) {
      throw new Error("identity is not initialized");
    }
    return this.state.identity;
  }

  private group(groupId: string): RuntimeGroupState {
    const existing = this.state.groups[groupId];
    if (existing) {
      return existing;
    }
    const created = normalizeGroupState(groupId, { group_id: groupId });
    this.state.groups[groupId] = created;
    return created;
  }
}

function decodeScaffoldCiphertext(ciphertextB64: string): string | null {
  const trimmed = ciphertextB64.trim();
  if (!trimmed) {
    return null;
  }
  try {
    const decoded = atob(trimmed);
    const bytes = Uint8Array.from(decoded, (char) => char.charCodeAt(0));
    const payload = JSON.parse(new TextDecoder().decode(bytes)) as { content?: unknown };
    const content = String(payload.content ?? "").trim();
    return content || null;
  } catch {
    return null;
  }
}
