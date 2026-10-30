# Google sync

Pangan syncs through the user's own Google Drive. There is no Pangan server: the
app talks to Google directly, and the data lives in a hidden per-app folder
(`appDataFolder`) inside the signed-in account's Drive. Nothing is visible in the
normal Drive UI, and nothing passes through us.

Sign in once on each device with the same Google account and everything follows:
focus history, tasks, and settings.

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

or paste it into the app: Settings → **Cloud** → the client-id field, which
appears when the build has no id compiled in. That path exists so a user can get
sync working without a rebuild.

## Signing in

Settings → **Cloud** → **Connect Google**. The app shows a code like
`ABCD-EFGH`; open <https://www.google.com/device> on any device, type the code,
and approve. The app picks up the token within a few seconds and turns automatic
sync on.

Do this on each device. Same account, and the data is the same.

## What is stored, and where

- Tokens: `~/.config/pangan/sync.toml` (Linux), mode `0600`. The refresh token is
  a long-lived credential, so the file is owner-only.
- Device identity and settings stamps: `sync-state.json` in the same directory.
  This is what lets a device tell "the user changed this here" from "this arrived
  from my other machine". Deleting it is safe, but the device then re-bootstraps
  its settings from the cloud on the next sync.
- Data: one JSON file, `pangan-sync.json`, in the Drive `appDataFolder`.

To stop syncing, press **Disconnect**: the tokens are removed from disk and
automatic sync is switched off. The file in Drive is left alone, so signing in
again on another device restores everything.

## What syncs, and what does not

Settings are the part people notice. A sync that carries history but not
preferences means you set 50-minute blocks on the desktop, open the laptop, and
get 25 minutes and a different chime. So settings travel too — but not all of
them, because some describe the machine rather than the person.

**Syncs:** focus/short-break/long-break durations, long-break interval,
auto-start next, auto-start on launch, daily goal, theme, heritage era, language,
animations, sound on/off, sound theme, tick sound, volume.

**Stays on this machine:** global hotkeys, automation commands, compact mode,
screen-lock inhibit, overlay, and the sync switches themselves. A `pactl` command
or a `<Super>p` accelerator pushed from a Linux desktop to a Mac is at best inert
and at worst surprising. The list is a whitelist (`PORTABLE` in
`src/syncfile.rs`), so a new setting is machine-local until someone decides
otherwise.

## How conflicts are handled

There is no conflict dialog, because there is nothing to decide:

- **Focus sessions** are facts. They are unioned by `(timestamp, minutes)`, so a
  session recorded on either device survives and one recorded on both is not
  doubled.
- **Tasks** carry `updated_at`; the newer copy wins.
- **Deletions** travel as tombstones (`deleted: true`) rather than as missing
  rows, so a delete on one device cannot be resurrected by another device that
  still has the old copy.
- **Settings** are merged per field, not as a block. Each field carries its own
  stamp, so changing the theme on the laptop does not revert the duration you set
  on the desktop a minute earlier. A field that has not changed keeps its old
  stamp and does not out-vote a real edit elsewhere just because this device
  synced more often.

Both merges are commutative and idempotent, so syncing often cannot drift.

## The write race

A merge resolves two snapshots. It cannot help if the file changes between the
read and the write, because the write would then replace a snapshot this device
never saw. That is a lost update, and no amount of merging recovers it.

So the upload carries the etag it read (`If-Match`). Drive rejects a stale write
with `412`, and the cycle re-reads and re-merges — folding in the other device's
work instead of overwriting it. Three attempts, then an honest error rather than
a silent overwrite.

## The file format

`pangan-sync.json` is an envelope:

```json
{
  "schema": 2,
  "revision": 14,
  "updated_at": 1793500000,
  "device_id": "9f2c...",
  "device_name": "laptop",
  "data": { "sessions": [], "tasks": [], "next_task_id": 1 },
  "settings": { "values": { "work_minutes": 50 }, "stamps": { "work_minutes": 1793499000 } }
}
```

- `schema` lets a reader refuse a file written by a newer build instead of
  silently dropping the fields it does not know about.
- `revision` increments on every write, so two devices can tell whether the file
  moved without diffing the payload.
- A file from the first release is a bare `Data` object with no `schema` key. It
  is read as "data, no settings" and migrated on the next write, so existing
  users keep their history.

## Troubleshooting

**"This build is not registered with Google."** No client id. See the setup
section above, or paste one in Settings → Cloud.

**"Sync error: …"** Check the network. A failed sync backs off (doubling up to
five minutes) instead of retrying every tick, so a dead network does not hammer
Drive or drain the battery.

**Signed out on its own.** A revoked or expired refresh token can never succeed,
so the app stops syncing and drops back to the signed-out state rather than
retrying forever behind a status line that says "error". Press **Connect Google**
again.

**Settings changed on their own.** That is the feature. The status line names the
device the change came from. If it is wrong, it is a merge bug, not a mystery —
the per-field stamps are in `sync-state.json`.
