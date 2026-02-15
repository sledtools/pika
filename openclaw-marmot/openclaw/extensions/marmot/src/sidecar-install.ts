import { constants, createWriteStream } from "node:fs";
import { access, chmod, mkdir, rename, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

type MarmotLog = {
  debug?: (msg: string) => void;
  info?: (msg: string) => void;
  warn?: (msg: string) => void;
  error?: (msg: string) => void;
};

type GitHubReleaseAsset = {
  name: string;
  browser_download_url: string;
};

type GitHubRelease = {
  tag_name: string;
  assets: GitHubReleaseAsset[];
};

// marmotd is built and released from this monorepo.
const DEFAULT_REPO = "justinmoon/pika";
const DEFAULT_BINARY_NAME = "marmotd";

function hasPathSeparator(input: string): boolean {
  return input.includes("/") || input.includes("\\");
}

async function isExecutableFile(filePath: string): Promise<boolean> {
  try {
    await access(filePath, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function resolveFromPath(binary: string): Promise<string | null> {
  const envPath = process.env.PATH ?? "";
  for (const dir of envPath.split(path.delimiter)) {
    const trimmed = dir.trim();
    if (!trimmed) continue;
    const candidate = path.join(trimmed, binary);
    if (await isExecutableFile(candidate)) {
      return candidate;
    }
  }
  return null;
}

async function resolveExistingCommand(cmd: string): Promise<string | null> {
  const trimmed = cmd.trim();
  if (!trimmed) return null;
  if (hasPathSeparator(trimmed)) {
    const absolute = path.resolve(trimmed);
    return (await isExecutableFile(absolute)) ? absolute : null;
  }
  return await resolveFromPath(trimmed);
}

function resolvePlatformAsset(): string {
  if (process.platform === "linux" && process.arch === "x64") return "marmotd-x86_64-linux";
  if (process.platform === "linux" && process.arch === "arm64") return "marmotd-aarch64-linux";
  if (process.platform === "darwin" && process.arch === "x64") return "marmotd-x86_64-darwin";
  if (process.platform === "darwin" && process.arch === "arm64") return "marmotd-aarch64-darwin";
  throw new Error(`unsupported platform for marmot auto-install: ${process.platform}/${process.arch}`);
}

function releaseApiUrl(repo: string, version: string): string {
  if (!version || version === "latest") {
    return `https://api.github.com/repos/${repo}/releases/latest`;
  }
  return `https://api.github.com/repos/${repo}/releases/tags/${encodeURIComponent(version)}`;
}

function releasesListApiUrl(repo: string, page: number): string {
  // Default ordering is newest-first.
  return `https://api.github.com/repos/${repo}/releases?per_page=50&page=${page}`;
}

function normalizeRelease(raw: any): GitHubRelease {
  const tagName = typeof raw?.tag_name === "string" ? raw.tag_name : "";
  const assets = Array.isArray(raw?.assets) ? raw.assets : [];
  const normalizedAssets: GitHubReleaseAsset[] = assets
    .map((a: any) => ({
      name: typeof a?.name === "string" ? a.name : "",
      browser_download_url:
        typeof a?.browser_download_url === "string" ? a.browser_download_url : "",
    }))
    .filter((a: GitHubReleaseAsset) => a.name && a.browser_download_url);

  if (!tagName) {
    throw new Error("release payload missing tag_name");
  }
  return { tag_name: tagName, assets: normalizedAssets };
}

async function fetchLatestReleaseWithAsset(params: {
  repo: string;
  assetName: string;
}): Promise<GitHubRelease> {
  const headers = new Headers({
    Accept: "application/vnd.github+json",
    "User-Agent": "openclaw-marmot-plugin",
  });
  const token = process.env.GITHUB_TOKEN?.trim();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }

  // Monorepo: "latest" release might belong to another component. Scan for the newest
  // release that includes the marmotd asset for this platform.
  for (let page = 1; page <= 4; page++) {
    const res = await fetch(releasesListApiUrl(params.repo, page), { headers });
    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new Error(`release list lookup failed ${res.status}: ${body.slice(0, 200)}`);
    }
    const list = (await res.json()) as any[];
    if (!Array.isArray(list) || list.length === 0) break;

    for (const raw of list) {
      const rel = normalizeRelease(raw);
      if (rel.assets.some((a) => a.name === params.assetName)) {
        return rel;
      }
    }
  }

  throw new Error(`no GitHub release found with asset ${params.assetName} in ${params.repo}`);
}

async function fetchRelease(params: {
  repo: string;
  version: string;
}): Promise<GitHubRelease> {
  if (!params.version || params.version === "latest") {
    return await fetchLatestReleaseWithAsset({
      repo: params.repo,
      assetName: resolvePlatformAsset(),
    });
  }

  const headers = new Headers({
    Accept: "application/vnd.github+json",
    "User-Agent": "openclaw-marmot-plugin",
  });
  const token = process.env.GITHUB_TOKEN?.trim();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }

  const res = await fetch(releaseApiUrl(params.repo, params.version), { headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`release lookup failed ${res.status}: ${body.slice(0, 200)}`);
  }
  return normalizeRelease(await res.json());
}

async function downloadFile(url: string, outPath: string): Promise<void> {
  const headers = new Headers({
    "User-Agent": "openclaw-marmot-plugin",
  });
  const token = process.env.GITHUB_TOKEN?.trim();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  const res = await fetch(url, {
    headers,
  });
  if (!res.ok || !res.body) {
    throw new Error(`download failed ${res.status} for ${url}`);
  }
  await pipeline(
    Readable.fromWeb(res.body as any),
    createWriteStream(outPath, { mode: 0o755 }),
  );
}

async function ensureInstalledBinary(params: {
  log?: MarmotLog;
  repo: string;
  version: string;
}): Promise<string> {
  const assetName = resolvePlatformAsset();
  const release = await fetchRelease({ repo: params.repo, version: params.version });
  const asset = release.assets.find((a) => a.name === assetName);
  if (!asset) {
    throw new Error(`release ${release.tag_name} missing asset ${assetName}`);
  }

  const installDir = path.join(os.homedir(), ".openclaw", "tools", "marmot", release.tag_name);
  const installedPath = path.join(installDir, DEFAULT_BINARY_NAME);
  if (await isExecutableFile(installedPath)) {
    params.log?.debug?.(`[marmot] using cached sidecar ${installedPath}`);
    return installedPath;
  }

  await mkdir(installDir, { recursive: true });
  const tempPath = path.join(
    installDir,
    `${DEFAULT_BINARY_NAME}.tmp-${process.pid}-${Date.now()}`,
  );

  try {
    params.log?.info?.(`[marmot] downloading sidecar asset ${assetName} (${release.tag_name})`);
    await downloadFile(asset.browser_download_url, tempPath);
    await chmod(tempPath, 0o755);
    await rename(tempPath, installedPath);
    return installedPath;
  } catch (err) {
    try {
      await rm(tempPath, { force: true });
    } catch {
      // ignore cleanup errors
    }
    if (await isExecutableFile(installedPath)) {
      return installedPath;
    }
    throw err;
  }
}

export async function resolveMarmotSidecarCommand(params: {
  requestedCmd: string;
  log?: MarmotLog;
}): Promise<string> {
  const existing = await resolveExistingCommand(params.requestedCmd);
  if (existing) return existing;

  params.log?.warn?.(
    `[marmot] sidecar command not found (${params.requestedCmd}); attempting auto-install`,
  );

  const repo = process.env.MARMOT_SIDECAR_REPO?.trim() || DEFAULT_REPO;
  const version = process.env.MARMOT_SIDECAR_VERSION?.trim() || "latest";
  const installed = await ensureInstalledBinary({
    log: params.log,
    repo,
    version,
  });
  params.log?.info?.(`[marmot] installed sidecar ${installed}`);
  return installed;
}
