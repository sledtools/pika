type MediaLike = {
  filename: string;
  mime_type: string;
  width?: number | null;
  height?: number | null;
  local_path?: string | null;
  url?: string;
};

export function augmentMessageText(content: string, media: MediaLike[] = []): string {
  if (!media.length) return content;
  const mediaLines = media.map((item) => {
    const dims = item.width && item.height ? ` (${item.width}x${item.height})` : "";
    const localFile = item.local_path ? ` file://${item.local_path}` : "";
    const url = !item.local_path && item.url ? ` ${item.url}` : "";
    return `[Attachment: ${item.filename} — ${item.mime_type}${dims}${localFile}${url}]`;
  });
  return content ? `${content}\n${mediaLines.join("\n")}` : mediaLines.join("\n");
}

export function sanitizeMeta(input: Record<string, string | undefined | null>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(input)) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
    const normalized = String(value ?? "").trim();
    if (!normalized) continue;
    out[key] = normalized;
  }
  return out;
}

export function detectMention(params: {
  text: string;
  botPubkey: string;
  botNpub: string;
  mentionPatterns: string[];
}): boolean {
  const text = params.text.toLowerCase();
  const pubkey = params.botPubkey.toLowerCase();
  const npub = params.botNpub.toLowerCase();

  if (npub && (text.includes(`nostr:${npub}`) || text.includes(npub))) {
    return true;
  }
  if (pubkey && (text.includes(`@${pubkey}`) || text.includes(pubkey))) {
    return true;
  }
  for (const pattern of params.mentionPatterns) {
    try {
      if (new RegExp(pattern, "i").test(params.text)) {
        return true;
      }
    } catch {
      // ignore invalid regex entries
    }
  }
  return false;
}
