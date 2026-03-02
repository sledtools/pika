# pika-news

`pika-news` turns pull request diffs into browser-first tutorial artifacts.

## Config file (`pika-news.toml`)

```toml
repos = ["sledtools/pika"]
poll_interval_secs = 60
model = "claude-sonnet-4-5-20250929"
api_key_env = "ANTHROPIC_API_KEY"
bind_address = "127.0.0.1"
bind_port = 8787
```

- `repos`: explicit `owner/repo` list for hosted polling mode.
- `poll_interval_secs`: interval used by hosted mode ingestion loop.
- `model`: Anthropic model name for tutorial generation.
- `api_key_env`: environment variable containing the API key.
- `bind_address`: hosted server bind address.
- `bind_port`: hosted server bind port.
