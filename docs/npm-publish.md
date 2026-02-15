---
summary: Runbook for publishing the Marmot plugin package to npm with passkey/browser 2FA.
read_when:
  - when publishing a new @justinmoon/marmot version
  - when npm publish prompts for browser authentication or OTP
---

# npm publish

Package:
- `@justinmoon/marmot`

## Standard publish flow

```sh
cd openclaw/extensions/marmot
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
   - `+ @justinmoon/marmot@<version>`

## Post-publish smoke check

```sh
openclaw plugins install @justinmoon/marmot
```
