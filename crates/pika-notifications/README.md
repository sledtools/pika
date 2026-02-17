# pika-notifications

Push notification server for Pika. Listens for Nostr kind 445 events filtered by `#h` tags (group IDs) and sends push notifications via APNs (iOS) and FCM (Android).

## Setup

Copy `.env.sample` to `.env` and fill in the values:

```bash
cp .env.sample .env
```

### Required

- `DATABASE_URL` — Postgres connection string
- `RELAYS` — Comma-separated list of Nostr relay WebSocket URLs to listen on

### Optional (push delivery)

Without these, the server logs notifications instead of sending them.

- `APNS_KEY_PATH` — Path to `.p8` key file from Apple Developer Portal
- `APNS_KEY_ID` — Key ID from Apple Developer Portal
- `APNS_TEAM_ID` — Team ID from Apple Developer account
- `APNS_TOPIC` — Bundle ID of the iOS app
- `FCM_CREDENTIALS_PATH` — Path to Firebase service account JSON

## Running

```bash
cargo run
```

Set `RUST_LOG=info` (or `trace` for maximum verbosity) to see notification pipeline logs.

## API

### `POST /register`

Register a device for push notifications.

```json
{
  "id": "unique-device-id",
  "device_token": "apns-or-fcm-token",
  "platform": "ios"
}
```

### `POST /subscribe-groups`

Subscribe a registered device to group IDs. The server will send a push when a kind 445 event with a matching `#h` tag arrives.

```json
{
  "id": "unique-device-id",
  "group_ids": ["group-abc", "group-xyz"]
}
```

### `POST /broadcast`

Send a notification to all registered devices.

```json
{
  "title": "Hello",
  "body": "This is a broadcast message"
}
```

### `GET /health-check`

Returns `200 OK`.
