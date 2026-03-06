import fs from "node:fs";
import path from "node:path";

import type {
  AgentBackend,
  AgentPromptResult,
  AgentSession,
  CreateAgentSessionInput,
  NormalizedAgentEvent,
  SessionEventType,
} from "./types.js";

const KNOWN_EVENT_TYPES = new Set<SessionEventType>([
  "agent_start",
  "turn_start",
  "message_start",
  "message_update",
  "tool_execution_start",
  "tool_execution_end",
  "turn_end",
  "agent_end",
  "steer",
  "error",
  "system",
]);

export class FakeAgentBackend implements AgentBackend {
  async createSession(input: CreateAgentSessionInput): Promise<AgentSession> {
    fs.mkdirSync(input.sessionDirectory, { recursive: true });
    const sessionPath = path.join(
      input.sessionDirectory,
      `issue-${input.issueNumber}-${Date.now()}.jsonl`,
    );
    fs.writeFileSync(sessionPath, "", "utf8");

    let aborted = false;
    const listeners = new Set<(event: NormalizedAgentEvent) => void>();

    const emit = (event: NormalizedAgentEvent): void => {
      fs.appendFileSync(sessionPath, `${JSON.stringify(event)}\n`, "utf8");
      for (const listener of listeners) {
        listener(event);
      }
    };

    return {
      sessionPath,
      subscribe(handler: (event: NormalizedAgentEvent) => void): () => void {
        listeners.add(handler);
        return () => {
          listeners.delete(handler);
        };
      },
      async prompt(_input: string): Promise<AgentPromptResult> {
        emit(event("agent_start", { backend: "fake" }));
        emit(event("turn_start", { turn: 1 }));
        emit(event("message_start", { role: "assistant" }));
        emit(event("message_update", { text: "Reading issue and planning implementation." }));

        if (aborted) {
          emit(event("error", { message: "Session aborted" }));
          throw new Error("session aborted");
        }

        const syntheticFile = path.join(input.workingDirectory, `.pika-dev-issue-${input.issueNumber}.md`);
        const syntheticContent = [
          `# Fake agent output for issue #${input.issueNumber}`,
          "",
          `Title: ${input.issueTitle}`,
          "",
          "This file is generated only when using `agent_backend = \"fake\"`.",
        ].join("\n");

        fs.mkdirSync(input.workingDirectory, { recursive: true });
        fs.writeFileSync(syntheticFile, `${syntheticContent}\n`, "utf8");
        emit(event("tool_execution_start", { tool: "write", file: path.basename(syntheticFile) }));
        emit(event("tool_execution_end", { tool: "write", ok: true }));

        emit(event("turn_end", { turn: 1 }));
        emit(event("agent_end", { summary: "Implemented fake change for local validation." }));

        return {
          summary: `Implemented a synthetic change for issue #${input.issueNumber} using the fake backend.`,
        };
      },
      async steer(message: string): Promise<void> {
        emit(event("steer", { mode: "steer", message }));
      },
      async followUp(message: string): Promise<void> {
        emit(event("steer", { mode: "followUp", message }));
      },
      async abort(reason?: string): Promise<void> {
        aborted = true;
        emit(event("error", { message: reason ?? "aborted" }));
      },
    };
  }
}

export class PiAgentBackend implements AgentBackend {
  constructor(
    private readonly provider: string,
    private readonly model: string,
    private readonly thinkingLevel: string,
  ) {}

  async createSession(input: CreateAgentSessionInput): Promise<AgentSession> {
    fs.mkdirSync(input.sessionDirectory, { recursive: true });

    const agentModule = (await import("@mariozechner/pi-coding-agent")) as Record<string, unknown>;
    const aiModule = (await import("@mariozechner/pi-ai")) as Record<string, unknown>;

    const getModel = aiModule.getModel as
      | ((provider: string, model: string) => unknown)
      | undefined;
    const createAgentSession = agentModule.createAgentSession as
      | ((options: Record<string, unknown>) => Promise<Record<string, unknown>>)
      | undefined;

    if (!getModel || !createAgentSession) {
      throw new Error("Pi SDK exports changed: expected getModel/createAgentSession");
    }

    const model = getModel(this.provider, this.model);

    const sessionManagerFactory = (agentModule.SessionManager as Record<string, unknown> | undefined)?.create;
    const sessionManager =
      typeof sessionManagerFactory === "function"
        ? (sessionManagerFactory as (cwd: string, sessionDir?: string) => unknown)(
            input.workingDirectory,
            input.sessionDirectory,
          )
        : undefined;

    const created = await createAgentSession({
      cwd: input.workingDirectory,
      model,
      thinkingLevel: this.thinkingLevel,
      sessionManager,
    });

    const rawSession = (created.session ?? created) as Record<string, unknown>;

    let sessionPath: string | null = null;
    if (typeof rawSession.sessionPath === "string") {
      sessionPath = rawSession.sessionPath;
    } else if (
      sessionManager &&
      typeof (sessionManager as Record<string, unknown>).getSessionFile === "function"
    ) {
      const file = ((sessionManager as Record<string, unknown>).getSessionFile as () => string | undefined)();
      if (typeof file === "string" && file.length > 0) {
        sessionPath = file;
      }
    }

    return {
      sessionPath,
      subscribe(handler: (event: NormalizedAgentEvent) => void): () => void {
        const subscribe = getSessionMethod<[(value: unknown) => void], (() => void) | void>(
          rawSession,
          "subscribe",
        );
        if (!subscribe) {
          return () => {};
        }

        const unsubscribe = subscribe((value: unknown) => {
          handler(normalizePiEvent(value));
        });

        if (typeof unsubscribe === "function") {
          return unsubscribe;
        }
        return () => {};
      },
      async prompt(inputPrompt: string): Promise<AgentPromptResult> {
        const prompt = getSessionMethod<[string], Promise<unknown>>(rawSession, "prompt");
        if (!prompt) {
          throw new Error("Pi session missing prompt() method");
        }
        const result = await prompt(inputPrompt);
        return {
          summary: extractSummary(result),
        };
      },
      async steer(message: string): Promise<void> {
        const steer = getSessionMethod<[string], Promise<void>>(rawSession, "steer");
        if (!steer) {
          throw new Error("Pi session missing steer() method");
        }
        await steer(message);
      },
      async followUp(message: string): Promise<void> {
        const followUp = getSessionMethod<[string], Promise<void>>(rawSession, "followUp");
        if (!followUp) {
          throw new Error("Pi session missing followUp() method");
        }
        await followUp(message);
      },
      async abort(reason?: string): Promise<void> {
        const abort = getSessionMethod<[string?], Promise<void>>(rawSession, "abort");
        if (!abort) {
          return;
        }
        await abort(reason);
      },
    };
  }
}

export function getSessionMethod<TArgs extends unknown[], TResult>(
  session: Record<string, unknown>,
  methodName: string,
): ((...args: TArgs) => TResult) | undefined {
  const candidate = session[methodName];
  if (typeof candidate !== "function") {
    return undefined;
  }

  const fn = candidate as (...args: TArgs) => TResult;
  return (...args: TArgs) => fn.apply(session, args);
}

function event(type: SessionEventType, payload: Record<string, unknown>): NormalizedAgentEvent {
  return {
    type,
    payload,
    createdAt: new Date().toISOString(),
  };
}

function normalizePiEvent(raw: unknown): NormalizedAgentEvent {
  if (!raw || typeof raw !== "object") {
    return event("system", { raw });
  }

  const record = raw as Record<string, unknown>;
  const rawType = typeof record.type === "string" ? record.type : "system";
  const type = KNOWN_EVENT_TYPES.has(rawType as SessionEventType)
    ? (rawType as SessionEventType)
    : "system";

  return {
    type,
    payload: record,
    createdAt: new Date().toISOString(),
  };
}

function extractSummary(result: unknown): string {
  if (typeof result === "string") {
    return result;
  }

  if (result && typeof result === "object") {
    const record = result as Record<string, unknown>;
    if (typeof record.summary === "string" && record.summary.length > 0) {
      return record.summary;
    }
    if (typeof record.text === "string" && record.text.length > 0) {
      return record.text;
    }
    if (typeof record.output === "string" && record.output.length > 0) {
      return record.output;
    }
  }

  return "Agent finished without a structured summary.";
}
