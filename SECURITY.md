# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.7.x   | ✅ Current |
| 0.4.x   | ✅ Security fixes only |
| < 0.4   | ❌ No longer supported |

## Reporting a Vulnerability

If you discover a security vulnerability in DepoAudio, please report it responsibly:

1. **Open a GitHub Issue** at [github.com/mayes/depo-audio/issues](https://github.com/mayes/depo-audio/issues) with the label `security`
2. **Do not** include exploit details in public issues — just note that it's security-related and we'll coordinate privately

We aim to acknowledge reports within 48 hours and provide a fix or mitigation plan within 7 days for confirmed vulnerabilities.

## Scope

DepoAudio processes audio files locally on your machine. It does not transmit audio data to any external servers. The only network activity is:
- **Auto-update checks** — fetches `latest.json` from GitHub Releases to check for new versions
- **FFmpeg sidecars** — bundled locally, no network calls during conversion
