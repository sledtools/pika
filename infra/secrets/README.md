# Secrets

Encrypted via [sops](https://github.com/getsops/sops) + age (YubiKey-backed).

## pika-server.yaml

Required keys:
- `apns_key` -- Contents of the .p8 APNs key file
- `apns_key_id` -- APNs Key ID from Apple Developer Portal
- `apns_team_id` -- Apple Developer Team ID
- `fcm_credentials` -- Contents of the Firebase service account JSON

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

## microVM host notes

The `pika-build` microVM host stack (`vm-spawner` + `microvm.nix`) does not currently
require an additional sops secret for agent LLM keys. The `pika-cli agent new --provider microvm`
flow forwards provider keys directly from the local environment to the guest process.

MicroVM home backups use `restic` with a host-local env file instead of sops:

- `/etc/microvm-backup.env`
- Example template is installed at `/etc/microvm-backup.env.example`
