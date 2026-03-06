export type SessionStatus =
  | "queued"
  | "starting"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

export type SessionEventType =
  | "agent_start"
  | "turn_start"
  | "message_start"
  | "message_update"
  | "tool_execution_start"
  | "tool_execution_end"
  | "turn_end"
  | "agent_end"
  | "steer"
  | "error"
  | "system";

export type ChatRole = "user" | "assistant" | "system";

export interface IssueRecord {
  id: number;
  issue_number: number;
  title: string;
  body: string;
  author: string;
  labels: string;
  state: string;
  github_updated_at: string;
  last_polled_at: string;
  created_at: string;
  updated_at: string;
}

export interface SessionRecord {
  id: number;
  issue_id: number;
  status: SessionStatus;
  branch_name: string | null;
  worktree_path: string | null;
  pi_session_path: string | null;
  pr_number: number | null;
  pr_url: string | null;
  error_message: string | null;
  started_at: string | null;
  completed_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface SessionEventRecord {
  id: number;
  session_id: number;
  event_type: SessionEventType;
  payload: string;
  created_at: string;
}

export interface ChatMessageRecord {
  id: number;
  session_id: number;
  role: ChatRole;
  npub: string | null;
  content: string;
  created_at: string;
}

export interface SessionWithIssue {
  session: SessionRecord;
  issue: IssueRecord;
}

export interface DashboardSnapshot {
  activeCount: number;
  maxConcurrent: number;
  running: SessionWithIssue[];
  queued: SessionWithIssue[];
  completed: SessionWithIssue[];
}

export interface NormalizedAgentEvent {
  type: SessionEventType;
  payload: Record<string, unknown>;
  createdAt: string;
}

export interface AgentPromptResult {
  summary: string;
}

export interface AgentSession {
  sessionPath: string | null;
  subscribe(handler: (event: NormalizedAgentEvent) => void): () => void;
  prompt(input: string): Promise<AgentPromptResult>;
  steer(input: string): Promise<void>;
  followUp(input: string): Promise<void>;
  abort(reason?: string): Promise<void>;
}

export interface CreateAgentSessionInput {
  issueNumber: number;
  issueTitle: string;
  issueBody: string;
  workingDirectory: string;
  sessionDirectory: string;
  systemPrompt: string;
}

export interface AgentBackend {
  createSession(input: CreateAgentSessionInput): Promise<AgentSession>;
}

export interface GitHubIssue {
  number: number;
  title: string;
  body: string;
  state: "open" | "closed";
  user: string;
  labels: string[];
  updatedAt: string;
  createdAt: string;
}
