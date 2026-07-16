# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 1.0.x   | ✅ Current |
| 0.8.x   | ✅ Security fixes only |
| < 0.8   | ❌ No longer supported |

## Reporting a Vulnerability

If you discover a security vulnerability in DepoAudio, please report it responsibly:

1. **Open a GitHub Issue** at [github.com/mayes/depo-audio/issues](https://github.com/mayes/depo-audio/issues) with the label `security`
2. **Do not** include exploit details in public issues — just note that it's security-related and we'll coordinate privately

We aim to acknowledge reports within 48 hours and provide a fix or mitigation plan within 7 days for confirmed vulnerabilities.

## Scope

DepoAudio processes audio files locally on your machine. It does not transmit audio data to any external servers. The only network activity is:
- **Optional AI model downloads** — fetches ONNX model files from GitHub Releases when you install a model from Settings (SHA-256 verified)
- **FFmpeg sidecars** — bundled locally, no network calls during conversion
