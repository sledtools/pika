import process from "node:process";

import type { PikaDevConfig } from "./config.js";
import type { GitHubIssue } from "./types.js";

interface GitHubIssueResponse {
  number: number;
  title: string;
  body: string | null;
  state: "open" | "closed";
  user?: { login?: string };
  labels?: Array<string | { name?: string }>;
  updated_at: string;
  created_at: string;
  pull_request?: unknown;
}

export interface GitHubClient {
  listOpenIssuesWithLabel(): Promise<GitHubIssue[]>;
  getIssue(issueNumber: number): Promise<GitHubIssue | null>;
}

export class GitHubApiClient implements GitHubClient {
  private readonly repoOwner: string;
  private readonly repoName: string;
  private readonly label: string;
  private readonly token: string;

  constructor(config: PikaDevConfig) {
    this.repoOwner = config.repoOwner;
    this.repoName = config.repoName;
    this.label = config.github_label;

    const token = process.env[config.github_token_env] ?? "";
    if (!token) {
      throw new Error(`missing GitHub token env var: ${config.github_token_env}`);
    }
    this.token = token;
  }

  async listOpenIssuesWithLabel(): Promise<GitHubIssue[]> {
    const url = new URL(`https://api.github.com/repos/${this.repoOwner}/${this.repoName}/issues`);
    url.searchParams.set("labels", this.label);
    url.searchParams.set("state", "open");
    url.searchParams.set("per_page", "100");

    const response = await this.requestJson<GitHubIssueResponse[]>(url.toString());
    return response
      .filter((issue) => issue.pull_request === undefined)
      .map((issue) => normalizeIssue(issue));
  }

  async getIssue(issueNumber: number): Promise<GitHubIssue | null> {
    const url = `https://api.github.com/repos/${this.repoOwner}/${this.repoName}/issues/${issueNumber}`;

    const response = await fetch(url, {
      headers: this.headers(),
    });

    if (response.status === 404) {
      return null;
    }

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`GitHub request failed (${response.status}): ${text}`);
    }

    const payload = (await response.json()) as GitHubIssueResponse;
    if (payload.pull_request !== undefined) {
      return null;
    }
    return normalizeIssue(payload);
  }

  private async requestJson<T>(url: string): Promise<T> {
    const response = await fetch(url, {
      headers: this.headers(),
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`GitHub request failed (${response.status}): ${text}`);
    }

    return (await response.json()) as T;
  }

  private headers(): Record<string, string> {
    return {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${this.token}`,
      "User-Agent": "pika-dev/0.1",
      "X-GitHub-Api-Version": "2022-11-28",
    };
  }
}

function normalizeIssue(issue: GitHubIssueResponse): GitHubIssue {
  return {
    number: issue.number,
    title: issue.title,
    body: issue.body ?? "",
    state: issue.state,
    user: issue.user?.login ?? "unknown",
    labels: normalizeLabels(issue.labels ?? []),
    updatedAt: issue.updated_at,
    createdAt: issue.created_at,
  };
}

function normalizeLabels(labels: Array<string | { name?: string }>): string[] {
  return labels
    .map((label) => {
      if (typeof label === "string") {
        return label;
      }
      return label.name ?? "";
    })
    .filter((label) => label.length > 0);
}
