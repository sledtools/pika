import type { DashboardSnapshot, SessionWithIssue } from "../types.js";
import { escapeHtml, renderLayout } from "./layout.js";

export function renderDashboard(snapshot: DashboardSnapshot): string {
  const body = `
    <section class="panel">
      <h1>pika-dev</h1>
      <p>Active sessions: <strong>${snapshot.activeCount}</strong> / ${snapshot.maxConcurrent}</p>
    </section>

    ${renderSection("Running", snapshot.running, renderRunningCard)}
    ${renderSection("Queued", snapshot.queued, renderQueuedCard)}
    ${renderSection("Completed", snapshot.completed, renderCompletedCard)}
  `;

  return renderLayout("pika-dev dashboard", body);
}

function renderSection(
  title: string,
  sessions: SessionWithIssue[],
  renderer: (session: SessionWithIssue) => string,
): string {
  const cards = sessions.length === 0
    ? `<p class="muted">No ${title.toLowerCase()} sessions.</p>`
    : sessions.map(renderer).join("\n");

  return `
    <section class="panel">
      <h2>${escapeHtml(title)}</h2>
      <div class="card-list">${cards}</div>
    </section>
  `;
}

function renderRunningCard(entry: SessionWithIssue): string {
  return renderCard(entry, [
    `Status: ${entry.session.status}`,
    `Started: ${formatTimestamp(entry.session.started_at)}`,
    `Branch: ${entry.session.branch_name ?? "(pending)"}`,
  ]);
}

function renderQueuedCard(entry: SessionWithIssue): string {
  return renderCard(entry, [
    `Status: queued`,
    `Queued: ${formatTimestamp(entry.session.created_at)}`,
  ]);
}

function renderCompletedCard(entry: SessionWithIssue): string {
  const pr = entry.session.pr_url
    ? `<a href="${escapeHtml(entry.session.pr_url)}" target="_blank" rel="noopener">PR</a>`
    : "no PR";
  return renderCard(entry, [
    `Status: ${entry.session.status}`,
    `Completed: ${formatTimestamp(entry.session.completed_at)}`,
    `Result: ${pr}`,
  ]);
}

function renderCard(entry: SessionWithIssue, details: string[]): string {
  const issue = entry.issue;
  const detailList = details.map((detail) => `<li>${detail}</li>`).join("\n");

  return `
    <article class="card">
      <h3>#${issue.issue_number} ${escapeHtml(issue.title)}</h3>
      <ul>${detailList}</ul>
      <a href="/session/${entry.session.id}">View session</a>
    </article>
  `;
}

function formatTimestamp(input: string | null): string {
  if (!input) {
    return "n/a";
  }
  return escapeHtml(input);
}
