import { DEFAULT_ACCOUNT_ID, type OpenClawConfig } from "openclaw/plugin-sdk";
import { resolvePikachatChannelConfig, type PikachatChannelConfig } from "./config.js";

export type ResolvedPikachatAccount = {
  accountId: string;
  name: string | null;
  enabled: boolean;
  configured: boolean;
  config: PikachatChannelConfig;
};

function normalizeAccountId(accountId?: string | null): string {
  const trimmed = String(accountId ?? "").trim();
  return trimmed ? trimmed : DEFAULT_ACCOUNT_ID;
}

export function listPikachatAccountIds(cfg: OpenClawConfig): string[] {
  const accounts = (cfg.channels?.["pikachat-openclaw"] as any)?.accounts;
  if (accounts && typeof accounts === "object") {
    const ids = Object.keys(accounts)
      .map((k) => normalizeAccountId(k))
      .filter(Boolean);
    if (ids.length > 0) {
      // Include default account alongside named accounts
      const set = new Set([DEFAULT_ACCOUNT_ID, ...ids]);
      return [...set].toSorted();
    }
  }
  return [DEFAULT_ACCOUNT_ID];
}

export function resolveDefaultPikachatAccountId(_cfg: OpenClawConfig): string {
  return DEFAULT_ACCOUNT_ID;
}

export function resolvePikachatAccount(params: {
  cfg: OpenClawConfig;
  accountId?: string | null;
}): ResolvedPikachatAccount {
  const accountId = normalizeAccountId(params.accountId);
  const rawChannel = (params.cfg.channels?.["pikachat-openclaw"] ?? {}) as Record<string, unknown>;
  const rawAccount =
    (rawChannel as any)?.accounts && typeof (rawChannel as any).accounts === "object"
      ? ((rawChannel as any).accounts[accountId] as Record<string, unknown> | undefined)
      : undefined;
  const merged = {
    ...rawChannel,
    ...(rawAccount ?? {}),
  };

  const config = resolvePikachatChannelConfig(merged);
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
