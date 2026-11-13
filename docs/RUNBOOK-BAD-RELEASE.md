# Runbook: a bad release

Use this when a published DepoAudio release damages recordings, breaks
conversion, ships something it must not (for example a file the licence ledger
blocks), or has a security problem. The release owner runs it. It needs no
second person.

DepoAudio rules that still apply during an incident:

- **Never replace a published asset.** A published installer name always keeps
  the same bytes. A fix is a new version number, built and published through
  the normal workflows.
- **Never move or reuse a release tag.**
- **Do not put case details, recordings or file names** in issues, release
  notes or advisories.

## 1. Contain it (same day)

1. **Write down what is known:** the affected version, the platforms, what
   goes wrong, and whether anyone's audio or transcript could be harmed.
2. **Stop new downloads of the bad build.** Point "latest" back at the last good
   release. GitHub's `releases/latest` page (the README's download link) and
   `releases/latest/download/...` URLs follow it; the website's own links are
   handled in step 6:

   ```bash
   gh release edit vGOOD --latest
   ```

   Installed apps are not downgraded: the updater installs only a version newer
   than the one already installed.
3. **Warn people at the top of the bad release's notes.** Say what is wrong, who
   is affected, what to do now (for example "keep your source recordings; do
   not delete them until outputs are checked") and which version to use
   instead. Edit the notes only; do not touch the assets.

   ```bash
   gh release view vBAD --json body --jq .body > notes.md   # then add the warning at the top
   gh release edit vBAD --notes-file notes.md
   ```

4. **If the release must stop being downloadable at all** (a blocked file, or a
   security flaw that is being exploited), the owner can delete the affected
   assets with `gh release delete-asset vBAD <asset>`. That breaks every link to
   them, so record the reason in `CHANGELOG.md`. Do not upload anything back
   under the same name.
5. **If signed updates are live** (a release carried `latest.json`), check what
   the latest release's `latest.json` points to. Installed apps fetch
   `releases/latest/download/latest.json`, so step 2 also stops the bad build
   being offered as an update. If the bad build is already installed, the only
   fix is a newer version.
6. **Tell the website owner** (`depo-audio-website`) the same day, so download
   pages, release facts and checksums stop pointing at the bad build.
7. **For a security problem**, open a draft GitHub security advisory on this
   repository (the private route in `SECURITY.md`). Publish it with the fix.

## 2. Replace it

1. Fix the problem on `main` with a test that fails without the fix.
2. Bump the version (a patch number is enough) in `package.json`,
   `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`. Then run
   `npm run release:check`, which keeps the three versions equal.
3. Build and publish through the normal sequence in
   [`V1.0.3-RELEASE-CANDIDATE.md`](V1.0.3-RELEASE-CANDIDATE.md#exact-candidate-to-publication-sequence):
   `release.yml` builds a private draft, then `publish-release.yml` publishes it
   after the wait timer. The 24-hour wait still applies. Steps 1.2 and 1.3
   above are what protect people in the meantime.
4. Once the replacement is published and checked, put its version in the
   warning on the bad release, and confirm "latest" is the new release.

## 3. Close it

- Add a `CHANGELOG.md` entry that says what was wrong, which versions were
  affected, and what changed. Use plain words, and include no case content.
- Keep the bad release's page and notes as history (they explain the version
  gap), unless the owner decided to delete its assets in step 1.4.
- Add a test or release-contract check so the same thing cannot ship again, and
  note anything in this runbook that did not work.
