# Security Policy

## Supported Versions

| Version | Supported              |
| ------- | ---------------------- |
| 1.0.x   | ✅ Current             |
| < 1.0   | ❌ No longer supported |

## Reporting a Vulnerability

If you discover a security vulnerability in DepoAudio, please use GitHub's
[private vulnerability report](https://github.com/DepoStack/depo-audio/security/advisories/new).
Do not put exploit details, private recordings, filenames, case information, or
other sensitive material in a public issue. If the private report form is not
available, open a minimal issue in the
[canonical repository](https://github.com/DepoStack/depo-audio/issues) asking
for a private contact channel without disclosing the vulnerability.

Response timing depends on maintainer availability and the severity and scope
of the report. The repository does not promise a fixed response or remediation
deadline.

## Scope

DepoAudio processes audio files locally on your machine and does not upload
recordings for conversion, playback, analysis, or cleanup. Runtime network
activity in v1.0.3 is limited to features the user or release configuration
enables:

- **No learned-model network path** — v1.0.3 does not bundle, install,
  download, or execute learned-model files. Exact legacy app-data files can be
  listed only for user-initiated deletion.
- **Optional update checks and downloads** — available only when a published
  build contains a verified updater configuration. Releases without signed
  updater metadata keep this path dormant.
- **FFmpeg sidecars** — bundled locally; conversion does not download them or
  send recording data over the network.
