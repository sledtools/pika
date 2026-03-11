---
summary: Runbook for publishing the Pikachat plugin package to npm with passkey/browser 2FA.
read_when:
  - when publishing a new pikachat-openclaw version
  - when npm publish prompts for browser authentication or OTP
---

# npm publish

Package:
- `pikachat-openclaw`

## Standard publish flow

```sh
cd pikachat-openclaw/openclaw/extensions/pikachat-openclaw
npm login
npm publish --access public
```

## Passkey/browser 2FA flow

If npm prints:
- `Authenticate your account at: https://www.npmjs.com/auth/cli/...`
- `Press ENTER to open in the browser...`

Then:
1. Press Enter in the terminal.
2. Approve in browser/1Password passkey prompt.
3. Wait for success output:
  - `+ pikachat-openclaw@<version>`

## Post-publish smoke check

```sh
openclaw plugins install pikachat-openclaw
```
