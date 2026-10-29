# Google sync setup

Pangan syncs through the user's own Google Drive. There is no Pangan server:
the app talks to Google directly, and the data lives in a hidden per-app folder
(`appDataFolder`) inside the signed-in account's Drive.

## One-time: register the app with Google

Google will not issue tokens to an unregistered client, so this step has to be
done once by whoever builds the app. It takes about five minutes and costs
nothing.

1. Open <https://console.cloud.google.com/> and create a project (any name).
2. **APIs & Services → Library**, enable **Google Drive API**.
3. **APIs & Services → OAuth consent screen**:
   - User type: **External**.
   - Fill in the app name and the support email.
   - Add the scopes `.../auth/drive.appdata` and `.../auth/userinfo.email`.
   - While the app is in **Testing**, add your own Google account under
     **Test users**. Without this, Google refuses the sign-in.
4. **APIs & Services → Credentials → Create credentials → OAuth client ID**:
   - Application type: **TVs and Limited Input devices**. This is the type that
     supports the device flow; a "Desktop app" client will not work here.
   - Copy the client id. It ends in `.apps.googleusercontent.com`.

## Point the build at that client id

Either export it before building:

```sh
export PANGAN_GOOGLE_CLIENT_ID="1234...apps.googleusercontent.com"
cargo build --release
```

or replace the placeholder in `src/gdrive.rs` (`client_id()`), which is what a
release build should do so end users do not have to set anything.

## Signing in

Settings → **Cloud** → **Connect Google**. The app shows a code like
`ABCD-EFGH`; open <https://www.google.com/device> on any device, type the code,
and approve. The app picks up the token within a few seconds and turns
automatic sync on.

## What is stored, and where

- Tokens: `~/.config/pangan/sync.toml` (Linux), mode `0600`. The refresh token
  is a long-lived credential, so the file is owner-only.
- Data: one JSON file, `pangan-sync.json`, in the Drive `appDataFolder`. It is
  invisible in the Drive UI and cannot be edited by hand.

To stop syncing, press **Disconnect**: the tokens are removed from disk and
automatic sync is switched off. The file in Drive is left alone, so signing in
again on another device restores everything.

## How conflicts are handled

There is no conflict dialog, because there is nothing to decide:

- **Focus sessions** are facts. They are unioned by `(timestamp, minutes)`, so a
  session recorded on either device survives and one recorded on both is not
  doubled.
- **Tasks** carry `updated_at`; the newer copy wins.
- **Deletions** travel as tombstones (`deleted: true`) rather than as missing
  rows, so a delete on one device cannot be resurrected by another device that
  still has the old copy.

The merge is commutative and idempotent, so syncing often cannot drift.
