# Runbook: a lost or compromised signing or updater key

Use this when a key or credential that signs DepoAudio releases is lost, or
when someone else may have it. The release owner runs it.

**First, check what is actually in use.** Signing, notarization and updater
signatures are each switched on only when their secrets are configured (see
README, "Release signing configuration"). Read the signing status recorded for
each published release before you act. A key that never signed a published
release only needs rotating: no user has to do anything.

| Credential | Where it lives | What it protects |
| --- | --- | --- |
| Tauri updater private key | Actions secrets `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`), plus the owner's offline backup | Installed apps accept an update only if it is signed with this key. The matching public key is built into each installed app. |
| Apple Developer ID certificate + notarization login | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | macOS Gatekeeper trust for the `.dmg` and app |
| Windows Authenticode certificate | `WINDOWS_CERTIFICATE` (PFX), `WINDOWS_CERTIFICATE_PASSWORD`, `WINDOWS_CERTIFICATE_THUMBPRINT` | Windows SmartScreen and publisher trust for the MSI and EXE |
| GitHub access | owner account, tokens with access to this repository | Who can change workflows, secrets, the tag ruleset and releases |

## If a key may be compromised

1. **Stop new signing.** Delete the affected secrets from Settings → Secrets and
   variables → Actions, so no workflow run can use them. Do not dispatch
   `release.yml` until the new key is in place.
2. **Look for misuse.** Check the release list (drafts included) for releases or
   assets you did not make, the Actions run history for runs you did not start,
   the tag ruleset and the `release-publication` environment for changes, and
   the account's security log. If you find a bad release, follow
   [RUNBOOK-BAD-RELEASE.md](RUNBOOK-BAD-RELEASE.md) as well.
3. **Revoke or replace, by credential:**
   - **Updater key.** An attacker needs this key *and* the ability to publish
     `latest.json` on this repository's releases to push an update. Generate a
     new keypair (`npx tauri signer generate -w <offline path>`) and keep two
     encrypted offline copies. Ship one transition release signed with the
     **old** key that carries the **new** public key. After that release,
     update only with the new key. If the old key can no longer be trusted
     even for that, skip the transition release. Publish the new build without
     the old signature and tell users to install it by hand. Older installs
     then stop receiving updates.
   - **Apple certificate.** Revoke it in the Apple Developer account and issue a
     new Developer ID Application certificate. Before revoking, ask Apple how
     revocation affects copies already notarized and in use. Replace the
     app-specific password `APPLE_PASSWORD` at appleid.apple.com as well.
   - **Windows certificate.** Ask the certificate authority to revoke it, with
     the revocation date set to the first time it may have been misused.
     Releases timestamped before that date stay trusted. Get a new certificate,
     preferably held in the CA's cloud signing service rather than a PFX
     (audit DA-03).
   - **GitHub access.** Change the password, sign out of other sessions, remove
     unknown SSH keys, tokens and app authorizations, and re-check two-factor
     authentication. Then treat every Actions secret as exposed and replace it.
4. **Add the new secrets**, then build and publish the next version through the
   normal sequence. Record which key signed it in the release gate.
5. **Tell people** if a published build could be affected. Use a GitHub security
   advisory and a note on the affected release, and tell the website owner
   the same day.

## If a key is lost (no one else has it)

- **Updater key lost:** installed apps can never verify an update signed with a
  different key. Generate a new keypair with offline backups and ship the next
  version with the new public key. Users must download and install that
  version by hand once. Say so in its release notes and on the site.
- **Windows certificate lost:** ask the certificate authority to revoke it,
  with a revocation date later than the signing time of every published
  release, so it cannot be misused if it turns up. Then get a new one. Releases timestamped before
  that date stay trusted, so people who installed them need do nothing.
- **Apple certificate lost:** do not revoke it straight away. As in the
  compromise steps above, first ask Apple how revoking it affects copies
  already notarized and in use: revoking a Developer ID certificate can stop
  macOS from opening apps signed with it. Then decide whether to revoke and get
  a new one. Until Apple has answered, do not tell users that they need do
  nothing; if a revocation would affect installed copies, tell them to install
  the next release signed with the new certificate.

## Prevention

- Keep the updater private key in two encrypted offline copies, never only in
  Actions secrets. The README's warning ("losing it means you can't ship
  updates") is about this key.
- Scope signing secrets to the packaging steps, as `release.yml` does, and
  review who can change workflows and environments.
- `npm run ci:local` and the CI `secret-scan` job run gitleaks over the full
  history, so a key committed by mistake is caught before or right after it is
  pushed. A key that was pushed counts as compromised: follow this runbook.
