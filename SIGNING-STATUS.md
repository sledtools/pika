# iOS Signing Setup — Status & Handoff Notes

## What was done

### 1. Merged PR #110 (push notification support)
- Merged `benthecarman/pika:notifs` (PR #110) into the local `notifs` branch.
- PR adds: push notification server (`crates/pika-notifications/`), Notification Service Extension (`crates/pika-nse/`, `ios/NotificationService/`), push subscription logic in Rust (`rust/src/core/push.rs`), and iOS notification settings UI.

### 2. Fixed NSE bundle ID derivation (committed: `94c643b`)
- **Problem**: `tools/run-ios` passed `PRODUCT_BUNDLE_IDENTIFIER` on the xcodebuild command line, which overrode ALL targets — both the main app and the Notification Service Extension (NSE) got the same bundle ID, which is invalid.
- **Fix**: Introduced a custom build setting `PIKA_APP_BUNDLE_ID` in `ios/project.yml`. The main app sets `PRODUCT_BUNDLE_IDENTIFIER: $(PIKA_APP_BUNDLE_ID)` and the NSE sets `PRODUCT_BUNDLE_IDENTIFIER: $(PIKA_APP_BUNDLE_ID).notification-service`. `tools/run-ios` now passes `PIKA_APP_BUNDLE_ID=...` instead of `PRODUCT_BUNDLE_IDENTIFIER=...`.

### 3. Removed invalid entitlement from NSE (committed: `94c643b`)
- Removed `com.apple.developer.usernotifications.communication` from `ios/NotificationService/NotificationService.entitlements` — this entitlement is only valid for the main app target, not extensions.

### 4. Successfully built and launched with `com.justinmoon.pikatest` (committed: `94c643b`)
- Used team `W6VF934SEW` (Justin's own paid team) and bundle ID `com.justinmoon.pikatest`.
- App group: `group.com.justinmoon.pikatest` — registered on team W6VF934SEW via Xcode GUI.
- Keychain access group prefix: `W6VF934SEW`.
- App built, installed, and launched successfully on device `00008140-001E54E90E6A801C`.

### 5. Switched to `com.justinmoon.pika` (committed: `9d858cf`)
- Changed bundle ID back to `com.justinmoon.pika` in project.yml, entitlements, and Swift code.
- Changed app group back to `group.com.justinmoon.pika`.

### 6. Switching to Ben's team `6JWFWV65BL` (UNCOMMITTED — in working tree)
- The goal is to use Ben Carman's team (`6JWFWV65BL`) because he owns `group.com.justinmoon.pika` and the original keychain access group prefix `6JWFWV65BL`.
- Changed keychain access group prefix back to `6JWFWV65BL` in all 4 files.
- Added `DEVELOPMENT_TEAM: 6JWFWV65BL` and `CODE_SIGN_STYLE: Automatic` to `ios/project.yml`.
- Updated `.env` to `PIKA_IOS_DEVELOPMENT_TEAM=6JWFWV65BL`.

## Current blocker

**Xcode shows "No Account for Team 6JWFWV65BL"** even though:
- Ben (benthecarman@live.com) added Justin (jamoen7@gmail.com) as Admin on his App Store Connect team.
- Xcode Settings → Apple Accounts shows "Benjamin Carman (Admin)" under Justin's teams.
- But when the project has `DEVELOPMENT_TEAM=6JWFWV65BL` hardcoded, Xcode says "Unknown Name (6JWFWV65BL)" and can't find the account.

### Root cause (likely)
Ben added Justin to **App Store Connect** but may not have added him to the **Apple Developer portal** team. These are separate:
- **App Store Connect** (appstoreconnect.apple.com) — for app distribution, TestFlight, etc.
- **Apple Developer portal** (developer.apple.com) — for certificates, provisioning profiles, app IDs, etc.

Justin needs to be a member of the developer portal team that owns `6JWFWV65BL`. Ben needs to verify this at https://developer.apple.com/account — under People/Members for team `6JWFWV65BL`.

### Alternative possibility
`6JWFWV65BL` might be Ben's **App ID Prefix** (different from Team ID). The team Justin sees in Xcode as "Benjamin Carman" might have a different Team ID. To verify:
1. Click "Benjamin Carman" team in Xcode Settings → Apple Accounts
2. Note the Team ID shown there
3. If it's NOT `6JWFWV65BL`, we need to use that team ID instead and figure out what `6JWFWV65BL` actually is

## What needs to happen next

1. **Determine the correct team ID for Ben's team that Justin has access to**:
   - In Xcode → Settings → Apple Accounts → click "Benjamin Carman" → note the Team ID.
   - OR: ask Ben what his Team ID is at developer.apple.com/account (Membership Details).

2. **Ensure the App ID prefix matches**:
   - The keychain access group is hardcoded as `6JWFWV65BL.com.justinmoon.pika.shared` in 4 files.
   - The entitlements use `$(AppIdentifierPrefix)com.justinmoon.pika.shared` which resolves at build time.
   - These MUST match. Check the built app's actual entitlements with:
     ```
     codesign -d --entitlements :- ios/build-device/Build/Products/Debug-iphoneos/Pika.app
     ```

3. **Register `group.com.justinmoon.pika` on the correct team**:
   - This app group is already claimed by team `6JWFWV65BL`.
   - Whatever team you build with must own this app group.
   - Registration must happen through Xcode GUI (select team on Pika and PikaNotificationService targets, let Xcode auto-provision).

4. **Register the device on the correct team**:
   - Device UDID: `00008140-001E54E90E6A801C`
   - Must be registered at developer.apple.com/account/resources/devices under the team being used.

5. **Build command** (once signing is resolved):
   ```
   just run-ios --device --udid 00008140-001E54E90E6A801C
   ```

## Key files modified (uncommitted changes)

| File | Change |
|------|--------|
| `.env` | `PIKA_IOS_DEVELOPMENT_TEAM=6JWFWV65BL`, `PIKA_IOS_BUNDLE_ID=com.justinmoon.pika` |
| `ios/project.yml` | Added `DEVELOPMENT_TEAM: 6JWFWV65BL`, `CODE_SIGN_STYLE: Automatic`, `PIKA_APP_BUNDLE_ID` mechanism |
| `ios/Pika.entitlements` | App group = `group.com.justinmoon.pika` |
| `ios/NotificationService/NotificationService.entitlements` | App group = `group.com.justinmoon.pika`, removed `usernotifications.communication` |
| `ios/Sources/KeychainNsecStore.swift` | Access group prefix = `6JWFWV65BL` |
| `ios/Sources/AppManager.swift` | App group = `group.com.justinmoon.pika` |
| `ios/NotificationService/NotificationService.swift` | Access group prefix = `6JWFWV65BL`, app group = `group.com.justinmoon.pika` |
| `rust/src/mdk_support.rs` | Access group prefix = `6JWFWV65BL` |
| `crates/pika-nse/src/mdk_support.rs` | Access group prefix = `6JWFWV65BL` |
| `tools/run-ios` | Passes `PIKA_APP_BUNDLE_ID` instead of `PRODUCT_BUNDLE_IDENTIFIER` |

## Team/account summary

| Entity | ID | Notes |
|--------|----|-------|
| Ben's team (App Store Connect) | `W6VF934SEW` | Justin is Admin. App group `group.com.justinmoon.pikatest` registered here. |
| Ben's team (App ID prefix) | `6JWFWV65BL` | Owns `group.com.justinmoon.pika`. Hardcoded in keychain access groups. |
| Justin's paid team | `W6VF934SEW` | May be the same as Ben's ASC team? Or separate. Built successfully with `com.justinmoon.pikatest`. |
| Justin's personal team | (free) | Cannot use push notifications, app groups, etc. |

## What worked previously
Building with team `W6VF934SEW` and bundle ID `com.justinmoon.pikatest` worked end-to-end (build, install, launch on device). If the team situation with `6JWFWV65BL` can't be resolved, falling back to `com.justinmoon.pikatest` on team `W6VF934SEW` is a viable option — but the keychain prefix in code would need to be `W6VF934SEW` and the app group would be `group.com.justinmoon.pikatest`.
