import type { ChatMessageRecord, SessionEventRecord, SessionWithIssue } from "../types.js";
import { escapeHtml, renderLayout } from "./layout.js";

export interface SessionPageData {
  entry: SessionWithIssue;
  events: SessionEventRecord[];
  chatMessages: ChatMessageRecord[];
  authEnabled: boolean;
  viewerNpub: string | null;
}

export function renderSessionPage(data: SessionPageData): string {
  const { entry } = data;
  const issue = entry.issue;
  const session = entry.session;
  const readOnly = isTerminal(session.status);

  const authSummary = data.authEnabled
    ? data.viewerNpub
      ? `Authenticated as ${escapeHtml(data.viewerNpub)}`
      : "Chat/export require NIP-98 auth token"
    : "Auth disabled (local mode).";

  const body = `
    <section class="panel">
      <h1>Issue #${issue.issue_number}: ${escapeHtml(issue.title)}</h1>
      <p>Status: <strong>${session.status}</strong></p>
      <p>Branch: ${escapeHtml(session.branch_name ?? "(pending)")}</p>
      <p>Worktree: ${escapeHtml(session.worktree_path ?? "(pending)")}</p>
      <p>${authSummary}</p>
      <p>Fork locally: <code>just dev-fork ${issue.issue_number}</code></p>
      <p><a href="/session/${session.id}/export">Download session JSONL</a></p>
    </section>

    <section class="panel two-column">
      <div>
        <h2>Agent Events</h2>
        <ul id="events" class="events">
          ${data.events.map(renderEvent).join("\n")}
        </ul>
      </div>

      <div>
        <h2>Chat</h2>
        <ul id="chat" class="chat">
          ${data.chatMessages.map(renderChatMessage).join("\n")}
        </ul>

        ${readOnly ? `<p class="muted">Chat is read-only for terminal sessions.</p>` : `
          <form id="chat-form" class="chat-form">
            <label>
              Message
              <textarea name="message" rows="4" required placeholder="Steer the agent"></textarea>
            </label>
            <label>
              Mode
              <select name="mode">
                <option value="steer">steer (interrupt current work)</option>
                <option value="followUp">followUp (after current turn)</option>
              </select>
            </label>
            <button type="submit">Send</button>
          </form>
          <p id="chat-status" class="muted"></p>
        `}
      </div>
    </section>

    <script>
      (() => {
        const sessionId = ${session.id};
        const eventsEl = document.getElementById("events");
        const chatEl = document.getElementById("chat");
        const source = new EventSource("/session/${session.id}/events");

        source.addEventListener("session_event", (event) => {
          const payload = JSON.parse(event.data);
          const li = document.createElement("li");
          li.textContent = payload.created_at + " [" + payload.event_type + "] " + payload.payload;
          eventsEl.appendChild(li);
          eventsEl.scrollTop = eventsEl.scrollHeight;
        });

        source.addEventListener("chat_message", (event) => {
          const payload = JSON.parse(event.data);
          const li = document.createElement("li");
          li.textContent = payload.created_at + " " + payload.role + ": " + payload.content;
          chatEl.appendChild(li);
          chatEl.scrollTop = chatEl.scrollHeight;
        });

        const form = document.getElementById("chat-form");
        const status = document.getElementById("chat-status");
        if (!form) {
          return;
        }

        form.addEventListener("submit", async (e) => {
          e.preventDefault();
          const formData = new FormData(form);
          const message = String(formData.get("message") || "").trim();
          const mode = String(formData.get("mode") || "steer");
          if (!message) {
            status.textContent = "Message is required.";
            return;
          }

          status.textContent = "Sending...";
          const response = await fetch("/session/${session.id}/chat", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            credentials: "include",
            body: JSON.stringify({ message, mode }),
          });

          if (!response.ok) {
            const text = await response.text();
            status.textContent = "Send failed: " + text;
            return;
          }

          form.reset();
          status.textContent = "Sent.";
        });
      })();
    </script>
  `;

  return renderLayout(`pika-dev session ${session.id}`, body);
}

function renderEvent(event: SessionEventRecord): string {
  return `<li>${escapeHtml(event.created_at)} [${escapeHtml(event.event_type)}] ${escapeHtml(event.payload)}</li>`;
}

function renderChatMessage(message: ChatMessageRecord): string {
  const sender = message.npub ? ` (${message.npub})` : "";
  return `<li>${escapeHtml(message.created_at)} <strong>${escapeHtml(message.role)}</strong>${escapeHtml(sender)}: ${escapeHtml(message.content)}</li>`;
}

function isTerminal(status: string): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}
