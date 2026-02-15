import { DEFAULT_ACCOUNT_ID, type OpenClawConfig } from "openclaw/plugin-sdk";
import { resolveMarmotChannelConfig, type MarmotChannelConfig } from "./config.js";

export type ResolvedMarmotAccount = {
  accountId: string;
  name: string | null;
  enabled: boolean;
  configured: boolean;
  config: MarmotChannelConfig;
};

function normalizeAccountId(accountId?: string | null): string {
  const trimmed = String(accountId ?? "").trim();
  return trimmed ? trimmed : DEFAULT_ACCOUNT_ID;
}

export function listMarmotAccountIds(cfg: OpenClawConfig): string[] {
  const accounts = (cfg.channels?.marmot as any)?.accounts;
  if (accounts && typeof accounts === "object") {
    const ids = Object.keys(accounts)
      .map((k) => normalizeAccountId(k))
      .filter(Boolean);
    if (ids.length > 0) {
      return ids.toSorted();
    }
  }
  return [DEFAULT_ACCOUNT_ID];
}

export function resolveDefaultMarmotAccountId(_cfg: OpenClawConfig): string {
  return DEFAULT_ACCOUNT_ID;
}

export function resolveMarmotAccount(params: {
  cfg: OpenClawConfig;
  accountId?: string | null;
}): ResolvedMarmotAccount {
  const accountId = normalizeAccountId(params.accountId);
  const rawChannel = (params.cfg.channels?.marmot ?? {}) as Record<string, unknown>;
  const rawAccount =
    (rawChannel as any)?.accounts && typeof (rawChannel as any).accounts === "object"
      ? ((rawChannel as any).accounts[accountId] as Record<string, unknown> | undefined)
      : undefined;
  const merged = {
    ...rawChannel,
    ...(rawAccount ?? {}),
  };

  const config = resolveMarmotChannelConfig(merged);
  const enabled = rawAccount && typeof (rawAccount as any).enabled === "boolean" ? Boolean((rawAccount as any).enabled) : true;
  const name =
    rawAccount && typeof (rawAccount as any).name === "string" ? String((rawAccount as any).name) : null;

  const configured = Array.isArray(config.relays) && config.relays.some((r) => String(r).trim());
  return {
    accountId,
    name,
    enabled,
    configured,
    config,
  };
}
