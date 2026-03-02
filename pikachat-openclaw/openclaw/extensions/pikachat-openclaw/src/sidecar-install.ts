import { constants, createWriteStream } from "node:fs";
import { access, chmod, mkdir, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

type PikachatLog = {
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

const DEFAULT_REPO = "sledtools/pika";
const DEFAULT_BINARY_NAME = "pikachat";
const VERSION_CHECK_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours
const MAX_KEPT_VERSIONS = 2; // current + one previous

// ---------------------------------------------------------------------------
// Plugin version (used to constrain auto-updates to patch-level only)
// ---------------------------------------------------------------------------

let _pluginVersion: string | null = null;

function getPluginVersion(): string {
  if (_pluginVersion) return _pluginVersion;
  try {
    const pkgPath = path.resolve(
      path.dirname(new URL(import.meta.url).pathname),
      "..",
      "package.json",
    );
    const pkg = JSON.parse(require("node:fs").readFileSync(pkgPath, "utf-8"));
    _pluginVersion = typeof pkg.version === "string" ? pkg.version : "0.0.0";
  } catch {
    _pluginVersion = "0.0.0";
  }
  return _pluginVersion ?? "0.0.0";
}

// ---------------------------------------------------------------------------
// Version utilities
// ---------------------------------------------------------------------------

function parseVer(v: string): number[] {
  return v.replace(/^(pikachat-)?v/, "").split(".").map(Number);
}

export function compareVersionsDesc(a: string, b: string): number {
  const [aMaj = 0, aMin = 0, aPat = 0] = parseVer(a);
  const [bMaj = 0, bMin = 0, bPat = 0] = parseVer(b);
  return bMaj - aMaj || bMin - aMin || bPat - aPat;
}

export function isCompatibleVersion(candidate: string, pluginVersion: string): boolean {
  const [cMaj = 0, cMin = 0] = parseVer(candidate);
  const [pMaj = 0, pMin = 0] = parseVer(pluginVersion);
  return cMaj === pMaj && cMin === pMin;
}

// ---------------------------------------------------------------------------
// Path / command resolution
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

function resolvePlatformAsset(): string {
  if (process.platform === "linux" && process.arch === "x64") return "pikachat-x86_64-linux";
  if (process.platform === "linux" && process.arch === "arm64") return "pikachat-aarch64-linux";
  if (process.platform === "darwin" && process.arch === "x64") return "pikachat-x86_64-darwin";
  if (process.platform === "darwin" && process.arch === "arm64") return "pikachat-aarch64-darwin";
  throw new Error(`unsupported platform for pikachat auto-install: ${process.platform}/${process.arch}`);
}

// ---------------------------------------------------------------------------
// Cache directory
// ---------------------------------------------------------------------------

function getCacheDir(): string {
  return path.join(os.homedir(), ".openclaw", "tools", "pikachat");
}

function getBinaryPath(version: string): string {
  return path.join(getCacheDir(), version, DEFAULT_BINARY_NAME);
}

// ---------------------------------------------------------------------------
// GitHub helpers
// ---------------------------------------------------------------------------

function githubHeaders(): Headers {
  const headers = new Headers({
    Accept: "application/vnd.github+json",
    "User-Agent": "openclaw-pikachat-plugin",
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

// Monorepo: "latest" release might belong to another component (e.g. pika/v*).
// Scan release pages to find the newest compatible release that has our platform asset.
// Only patch-level updates are allowed: the release must share the same major.minor
// as the plugin version (e.g. plugin 0.5.x only accepts pikachat 0.5.y releases).
async function fetchLatestCompatibleRelease(params: {
  repo: string;
  assetName: string;
  pluginVersion: string;
  log: PikachatLog;
}): Promise<GitHubRelease> {
  const headers = githubHeaders();

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
      if (!rel.assets.some((a) => a.name === params.assetName)) continue;
      if (isCompatibleVersion(rel.tag_name, params.pluginVersion)) {
        return rel;
      }
      params.log.debug?.(
        `[pikachat] skipping ${rel.tag_name} (incompatible with plugin v${params.pluginVersion}, patch-only updates allowed)`,
      );
    }
  }

  throw new Error(
    `no compatible GitHub release found for plugin v${params.pluginVersion} with asset ${params.assetName} in ${params.repo}`,
  );
}

async function fetchReleaseByTag(params: {
  repo: string;
  version: string;
}): Promise<GitHubRelease> {
  const headers = githubHeaders();
  const res = await fetch(releaseByTagApiUrl(params.repo, params.version), { headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`release lookup failed ${res.status}: ${body.slice(0, 200)}`);
  }
  return normalizeRelease(await res.json());
}

async function fetchRelease(params: {
  repo: string;
  version: string;
  log: PikachatLog;
}): Promise<GitHubRelease> {
  if (!params.version || params.version === "latest") {
    const pluginVersion = getPluginVersion();
    return await fetchLatestCompatibleRelease({
      repo: params.repo,
      assetName: resolvePlatformAsset(),
      pluginVersion,
      log: params.log,
    });
  }
  return await fetchReleaseByTag(params);
}

// ---------------------------------------------------------------------------
// Version resolution with 24h cache
// ---------------------------------------------------------------------------

async function resolveVersion(
  log: PikachatLog,
  repo: string,
  pinnedVersion?: string,
): Promise<string> {
  if (pinnedVersion) {
    return pinnedVersion;
  }

  const cacheDir = getCacheDir();
  const cacheFile = path.join(cacheDir, ".latest-version");

  try {
    const fileStat = await stat(cacheFile);
    const age = Date.now() - fileStat.mtimeMs;
    if (age < VERSION_CHECK_TTL_MS) {
      const cached = (await readFile(cacheFile, "utf-8")).trim();
      if (cached) {
        log.info?.(`[pikachat] using cached latest version: ${cached} (checked ${Math.round(age / 60000)}m ago)`);
        return cached;
      }
    }
  } catch {
    // No cache file or unreadable
  }

  log.info?.("[pikachat] checking GitHub for latest pikachat release...");
  const release = await fetchRelease({ repo, version: "latest", log });
  const version = release.tag_name;

  await mkdir(cacheDir, { recursive: true });
  await writeFile(cacheFile, version, "utf-8");

  return version;
}

// ---------------------------------------------------------------------------
// Checksum verification
// ---------------------------------------------------------------------------

async function verifyChecksum(
  filePath: string,
  checksumUrl: string,
  log: PikachatLog,
): Promise<void> {
  const headers = githubHeaders();
  const res = await fetch(checksumUrl, { headers, redirect: "follow" });
  if (!res.ok) {
    if (res.status === 404) {
      log.warn?.(
        `[pikachat] checksum file not found (404), skipping verification for ${path.basename(filePath)}`,
      );
      return;
    }
    throw new Error(
      `failed to fetch checksum for ${path.basename(filePath)}: ${res.status} ${res.statusText}. ` +
        `This may indicate GitHub rate limiting or a server error.`,
    );
  }

  const expectedLine = (await res.text()).trim();
  const expectedHash = expectedLine.split(/\s+/)[0];

  const { createHash } = await import("node:crypto");
  const fileBuffer = await readFile(filePath);
  const actualHash = createHash("sha256").update(fileBuffer).digest("hex");

  if (actualHash !== expectedHash) {
    await rm(filePath, { force: true });
    throw new Error(
      `checksum mismatch for ${path.basename(filePath)}: ` +
        `expected ${expectedHash}, got ${actualHash}`,
    );
  }

  log.info?.(`[pikachat] checksum verified for ${path.basename(filePath)}`);
}

// ---------------------------------------------------------------------------
// Old version cleanup
// ---------------------------------------------------------------------------

async function cleanupOldVersions(
  currentVersion: string,
  log: PikachatLog,
): Promise<void> {
  const cacheDir = getCacheDir();

  let entries: string[];
  try {
    entries = await readdir(cacheDir);
  } catch {
    return;
  }

  // Match pikachat version directories (e.g. pikachat-v0.4.0)
  const versionDirs = entries
    .filter((e) => e.startsWith("pikachat-v") || e.startsWith("v"))
    .sort(compareVersionsDesc);

  if (versionDirs.length <= MAX_KEPT_VERSIONS) {
    return;
  }

  const toKeep = new Set<string>([currentVersion]);
  for (const dir of versionDirs) {
    if (toKeep.size >= MAX_KEPT_VERSIONS) break;
    toKeep.add(dir);
  }

  for (const dir of versionDirs) {
    if (toKeep.has(dir)) continue;
    const dirPath = path.join(cacheDir, dir);
    try {
      await rm(dirPath, { recursive: true, force: true });
      log.info?.(`[pikachat] cleaned up old version: ${dir}`);
    } catch {
      // Best-effort cleanup
    }
  }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

async function downloadFile(url: string, outPath: string): Promise<void> {
  const headers = githubHeaders();
  const res = await fetch(url, { headers, redirect: "follow" });
  if (!res.ok || !res.body) {
    throw new Error(`download failed ${res.status} for ${url}`);
  }
  await pipeline(
    Readable.fromWeb(res.body as any),
    createWriteStream(outPath, { mode: 0o755 }),
  );
}

// ---------------------------------------------------------------------------
// ensureBinary — the main entry point for auto-install
// ---------------------------------------------------------------------------

export interface DownloadResult {
  binaryPath: string;
  version: string;
}

async function ensureInstalledBinary(params: {
  log: PikachatLog;
  repo: string;
  pinnedVersion?: string;
}): Promise<DownloadResult> {
  const { log, repo } = params;

  const version = await resolveVersion(log, repo, params.pinnedVersion);
  const binaryPath = getBinaryPath(version);

  if (await isExecutableFile(binaryPath)) {
    log.debug?.(`[pikachat] pikachat ${version} already cached at ${binaryPath}`);
    await cleanupOldVersions(version, log);
    return { binaryPath, version };
  }

  const assetName = resolvePlatformAsset();
  const release = await fetchRelease({ repo, version, log });
  const asset = release.assets.find((a) => a.name === assetName);
  if (!asset) {
    throw new Error(`release ${release.tag_name} missing asset ${assetName}`);
  }

  const installDir = path.join(getCacheDir(), release.tag_name);
  const installedPath = path.join(installDir, DEFAULT_BINARY_NAME);

  await mkdir(installDir, { recursive: true });
  const tempPath = path.join(
    installDir,
    `${DEFAULT_BINARY_NAME}.tmp-${process.pid}-${Date.now()}`,
  );

  try {
    log.info?.(`[pikachat] downloading pikachat ${release.tag_name} (${assetName})...`);
    await downloadFile(asset.browser_download_url, tempPath);

    // Verify checksum if available (gracefully skips if no .sha256 file in release)
    const checksumAsset = release.assets.find((a) => a.name === `${assetName}.sha256`);
    if (checksumAsset) {
      await verifyChecksum(tempPath, checksumAsset.browser_download_url, log);
    } else {
      log.warn?.(`[pikachat] no checksum file found for ${assetName}, skipping verification`);
    }

    await chmod(tempPath, 0o755);
    await rename(tempPath, installedPath);
    log.info?.(`[pikachat] pikachat ${release.tag_name} ready at ${installedPath}`);
  } catch (err) {
    try {
      await rm(tempPath, { force: true });
    } catch {
      // ignore cleanup errors
    }
    if (await isExecutableFile(installedPath)) {
      return { binaryPath: installedPath, version: release.tag_name };
    }
    throw err;
  }

  await cleanupOldVersions(release.tag_name, log);
  return { binaryPath: installedPath, version: release.tag_name };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

export async function resolvePikachatSidecarCommand(params: {
  requestedCmd: string;
  log?: PikachatLog;
  pinnedVersion?: string;
}): Promise<string> {
  const log: PikachatLog = params.log ?? {};

  // If a custom sidecarCmd was explicitly configured (not the default "pikachat"),
  // resolve it directly — the user is in control.
  if (params.requestedCmd !== DEFAULT_BINARY_NAME) {
    const existing = await resolveExistingCommand(params.requestedCmd);
    if (existing) {
      log.info?.(`[pikachat] using configured sidecar command: ${existing}`);
      return existing;
    }
    log.warn?.(`[pikachat] configured sidecar command not found: ${params.requestedCmd}, falling back to auto-install`);
  }

  // Always use the managed binary location — download/update as needed.
  const repo = process.env.PIKACHAT_SIDECAR_REPO?.trim() || DEFAULT_REPO;
  const envVersion = process.env.PIKACHAT_SIDECAR_VERSION?.trim();
  const pinnedVersion = params.pinnedVersion ?? envVersion;

  const { binaryPath, version } = await ensureInstalledBinary({
    log,
    repo,
    pinnedVersion: pinnedVersion && pinnedVersion !== "latest" ? pinnedVersion : undefined,
  });
  log.info?.(`[pikachat] installed sidecar ${version} at ${binaryPath}`);
  return binaryPath;
}
