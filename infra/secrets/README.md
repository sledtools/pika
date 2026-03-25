# Secrets

Encrypted via [sops](https://github.com/getsops/sops) + age (YubiKey-backed).

## pika-server.yaml

Required keys:
- `apns_key` -- Contents of the .p8 APNs key file
- `apns_key_id` -- APNs Key ID from Apple Developer Portal
- `apns_team_id` -- Apple Developer Team ID
- `fcm_credentials` -- Contents of the Firebase service account JSON

Optional keys for an Incus mTLS canary on the normal `pika-server` deploy path:
- `incus_client_cert` -- PEM client certificate trusted by the remote Incus daemon
- `incus_client_key` -- PEM private key for `incus_client_cert`
- `incus_server_cert` -- PEM server certificate for the remote Incus daemon when not using `PIKA_AGENT_INCUS_INSECURE_TLS=true`
- `anthropic_api_key` -- Anthropic API key injected into managed OpenClaw guests provisioned by `pika-server`

`PIKA_ADMIN_SESSION_SECRET` is derived at runtime from the APNS private key hash, so no
separate admin session secret key is required in `pika-server.yaml`.

## Setup

1. After first deploy, SSH into the server and generate an age key:
   ```
   mkdir -p /etc/age && chmod 0700 /etc/age
   age-keygen -o /etc/age/key.txt && chmod 0400 /etc/age/key.txt
   age-keygen -y /etc/age/key.txt  # prints public key
   ```

2. Add the server's public key to `.sops.yaml` and re-encrypt:
   ```
   sops updatekeys infra/secrets/pika-server.yaml
   ```

3. Create the secrets file:
   ```
   sops infra/secrets/pika-server.yaml
   ```

## builder-cache-key.yaml

Required keys:
- `cache_signing_key` -- nix-serve binary cache signing secret key

Generate with:
```
nix-store --generate-binary-cache-key builder-cache builder-cache.sec builder-cache.pub
```

Then create the sops file:
```
sops infra/secrets/builder-cache-key.yaml
# Set cache_signing_key to the contents of builder-cache.sec
```

After first deploy, add the builder server's age public key to `.sops.yaml` and re-encrypt:
```
sops updatekeys infra/secrets/builder-cache-key.yaml
```
