export function renderLayout(title: string, body: string): string {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${escapeHtml(title)}</title>
    <link rel="stylesheet" href="/static/style.css" />
  </head>
  <body>
    <header class="site-header">
      <a href="/" class="brand">pika-dev</a>
      <a href="/health" class="health-link">health</a>
    </header>
    <main class="page">${body}</main>
  </body>
</html>`;
}

export function escapeHtml(input: string): string {
  return input
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}
