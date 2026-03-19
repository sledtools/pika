import { constants } from "node:fs";
import { access, chmod, mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import type { PikachatLogger } from "./daemon-client.js";

type GitHubReleaseAsset = {
  name: string;
  browser_download_url: string;
};

type GitHubRelease = {
  tag_name: string;
  assets: GitHubReleaseAsset[];
};

const DEFAULT_REPO = "sledtools/pika";
const DEFAULT_BINARY_NAME = "pikachat";
const VERSION_CHECK_TTL_MS = 24 * 60 * 60 * 1000;

let pluginVersionCache: string | null = null;

function parseVer(value: string): number[] {
  return value.replace(/^(pikachat-)?v/, "").split(".").map(Number);
}

function getPackageVersion(): string {
  if (pluginVersionCache) return pluginVersionCache;
  pluginVersionCache = "0.1.0";
  return pluginVersionCache;
}

export function isCompatibleVersion(candidate: string, pluginVersion: string): boolean {
  const [cMaj = 0, cMin = 0] = parseVer(candidate);
  const [pMaj = 0, pMin = 0] = parseVer(pluginVersion);
  return cMaj === pMaj && cMin === pMin;
}

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
  if (process.platform === "linux" && process.arch === "x64") return "pikachat-x86_64-linux";
  if (process.platform === "linux" && process.arch === "arm64") return "pikachat-aarch64-linux";
  if (process.platform === "darwin" && process.arch === "x64") return "pikachat-x86_64-darwin";
  if (process.platform === "darwin" && process.arch === "arm64") return "pikachat-aarch64-darwin";
  throw new Error(`unsupported platform for pikachat auto-install: ${process.platform}/${process.arch}`);
}

function getCacheDir(): string {
  return path.join(os.homedir(), ".claude", "channels", "pikachat", "tools");
}

function getBinaryPath(version: string): string {
  return path.join(getCacheDir(), version, DEFAULT_BINARY_NAME);
}

function githubHeaders(): Headers {
  const headers = new Headers({
    Accept: "application/vnd.github+json",
    "User-Agent": "pikachat-claude",
  });
  const token = process.env.GITHUB_TOKEN?.trim();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  return headers;
}

function releasesListApiUrl(repo: string, page: number): string {
  return `https://api.github.com/repos/${repo}/releases?per_page=50&page=${page}`;
}

function releaseByTagApiUrl(repo: string, version: string): string {
  return `https://api.github.com/repos/${repo}/releases/tags/${encodeURIComponent(version)}`;
}

function normalizeRelease(raw: any): GitHubRelease {
  const tagName = typeof raw?.tag_name === "string" ? raw.tag_name : "";
  const assets = Array.isArray(raw?.assets) ? raw.assets : [];
  const normalizedAssets: GitHubReleaseAsset[] = assets
    .map((entry: any) => ({
      name: typeof entry?.name === "string" ? entry.name : "",
      browser_download_url: typeof entry?.browser_download_url === "string" ? entry.browser_download_url : "",
    }))
    .filter((entry: GitHubReleaseAsset) => entry.name && entry.browser_download_url);
  if (!tagName) {
    throw new Error("release payload missing tag_name");
  }
  return { tag_name: tagName, assets: normalizedAssets };
}

async function fetchLatestCompatibleRelease(params: {
  repo: string;
  assetName: string;
  pluginVersion: string;
  log?: PikachatLogger;
}): Promise<GitHubRelease> {
  const headers = githubHeaders();
  for (let page = 1; page <= 4; page++) {
    const response = await fetch(releasesListApiUrl(params.repo, page), { headers });
    if (!response.ok) {
      const body = await response.text().catch(() => "");
      throw new Error(`release list lookup failed ${response.status}: ${body.slice(0, 200)}`);
    }
    const list = (await response.json()) as any[];
    if (!Array.isArray(list) || list.length === 0) break;
    for (const raw of list) {
      const release = normalizeRelease(raw);
      if (!release.assets.some((asset) => asset.name === params.assetName)) continue;
      if (isCompatibleVersion(release.tag_name, params.pluginVersion)) {
        return release;
      }
      params.log?.debug?.(
        `[pikachat-claude] skipping ${release.tag_name} (incompatible with plugin ${params.pluginVersion})`,
      );
    }
  }
  throw new Error(`no compatible release found for asset ${params.assetName}`);
}

async function fetchReleaseByTag(params: { repo: string; version: string }): Promise<GitHubRelease> {
  const response = await fetch(releaseByTagApiUrl(params.repo, params.version), { headers: githubHeaders() });
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(`release lookup failed ${response.status}: ${body.slice(0, 200)}`);
  }
  return normalizeRelease(await response.json());
}

async function resolveVersion(log?: PikachatLogger, pinnedVersion?: string): Promise<string> {
  if (pinnedVersion) {
    return pinnedVersion;
  }
  const cacheDir = getCacheDir();
  const cacheFile = path.join(cacheDir, ".latest-version");
  try {
    const raw = JSON.parse(await readFile(cacheFile, "utf8")) as { value?: string; checked_at?: number };
    if (
      typeof raw.value === "string" &&
      typeof raw.checked_at === "number" &&
      Date.now() - raw.checked_at < VERSION_CHECK_TTL_MS
    ) {
      return raw.value;
    }
  } catch {
    // ignore stale cache misses
  }

  const release = await fetchLatestCompatibleRelease({
    repo: DEFAULT_REPO,
    assetName: resolvePlatformAsset(),
    pluginVersion: getPackageVersion(),
    log,
  });
  await mkdir(cacheDir, { recursive: true });
  await writeFile(
    cacheFile,
    JSON.stringify({ value: release.tag_name, checked_at: Date.now() }),
    "utf8",
  );
  return release.tag_name;
}

async function downloadToFile(url: string, destination: string): Promise<void> {
  const response = await fetch(url, { headers: githubHeaders(), redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`download failed ${response.status}: ${url}`);
  }
  const tmpPath = `${destination}.tmp`;
  const buffer = Buffer.from(await response.arrayBuffer());
  await writeFile(tmpPath, buffer);
  await rename(tmpPath, destination);
}

export async function resolvePikachatDaemonCommand(params: {
  requestedCmd: string;
  pinnedVersion?: string;
  log?: PikachatLogger;
}): Promise<string> {
  const existing = await resolveExistingCommand(params.requestedCmd);
  if (existing) {
    return existing;
  }

  const requested = params.requestedCmd.trim();
  if (requested !== DEFAULT_BINARY_NAME) {
    throw new Error(`daemon command not found: ${requested}`);
  }

  const version = await resolveVersion(params.log, params.pinnedVersion);
  const binaryPath = getBinaryPath(version);
  if (await isExecutableFile(binaryPath)) {
    return binaryPath;
  }

  await mkdir(path.dirname(binaryPath), { recursive: true });
  const release =
    params.pinnedVersion && params.pinnedVersion !== "latest"
      ? await fetchReleaseByTag({ repo: DEFAULT_REPO, version: params.pinnedVersion })
      : await fetchLatestCompatibleRelease({
          repo: DEFAULT_REPO,
          assetName: resolvePlatformAsset(),
          pluginVersion: getPackageVersion(),
          log: params.log,
        });
  const asset = release.assets.find((entry) => entry.name === resolvePlatformAsset());
  if (!asset) {
    throw new Error(`release ${release.tag_name} missing asset ${resolvePlatformAsset()}`);
  }

  await downloadToFile(asset.browser_download_url, binaryPath);
  await chmod(binaryPath, 0o755);

  try {
    const fileStat = await stat(binaryPath);
    if (!fileStat.isFile()) {
      throw new Error(`downloaded path is not a file: ${binaryPath}`);
    }
  } catch (err) {
    await rm(binaryPath, { force: true });
    throw err;
  }
  return binaryPath;
}
