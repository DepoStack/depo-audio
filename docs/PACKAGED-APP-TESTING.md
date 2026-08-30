# Packaged-app automation design

Status: **deferred by dependency policy**. DepoAudio does not currently install or expose a WebdriverIO packaged-app runner. This document preserves the proposed test architecture and the evidence required to activate it without adding known high-severity dependency findings or changing the production Tauri binary.

## Dependency decision

Tauri's current packaged-app testing guidance uses `@wdio/tauri-service`, but the compatible dependency graph reviewed on August 24, 2026 did not pass `npm audit`.

| Vulnerable package           | Dependency path                                             | Advisory                                     | Supported patched path                                                                                                           |
| ---------------------------- | ----------------------------------------------------------- | -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `deepmerge-ts@7.1.6`         | WebdriverIO config, runner, utilities, and WebDriver client | `GHSA-ggr8-5vv4-36mx`                        | No. Version 8.0.2 is patched, but current WebdriverIO packages require `^7.0.3`.                                                 |
| `serialize-javascript@6.0.2` | `@wdio/mocha-framework` to Mocha                            | `GHSA-5c6j-r48x-rmvq`, `GHSA-qj8w-gfj5-8c6v` | No. Version 7.1.0 is patched, but current Mocha 10 and 11 releases require `^6.0.2`.                                             |
| `extract-zip@2.0.1`          | WebdriverIO utilities to `@puppeteer/browsers@2.13.2`       | `GHSA-jmr9-qjv8-65gv`                        | No. `extract-zip` has no patched release. `@puppeteer/browsers@3.2.1` removes it, but current WebdriverIO requires the 2.x line. |

The audit reported 15 high-severity and one moderate entry because those three vulnerable packages propagate through multiple WebdriverIO packages. Major-version overrides would be outside the maintainers' declared compatibility ranges and would leave the native runner unproven, so they are not an acceptable fix.

Reconsider the harness only when an exact compatible graph has no high or critical findings, or when each remaining finding has a narrowly documented, reviewed mitigation. Do not add a blanket audit exception.

## Intended architecture

- Use WebdriverIO with `@wdio/tauri-service` only after the dependency gate passes.
- Select the official external `tauri-driver` provider on Windows and Linux.
- Keep automatic `tauri-driver` installation disabled; install it from the repository's locked Rust toolchain.
- Point the runner at an immutable packaged executable when possible, not an unrelated development binary.
- Keep backend and frontend log capture off by default because errors may contain local paths.
- Do not register a WebDriver or command-mocking plugin in the normal Tauri application.
- Do not change the production CSP or default capability for test automation.

The first smoke subset should cover:

- product-shell startup and title;
- released Convert and Player navigation;
- accessible local file-selection controls without opening a native dialog;
- an automated WCAG 2 A/AA scan of the initial view that blocks serious and critical violations.

The suite should not convert or inspect a court recording, mutate the Library, install an update, or call an external service. v1.0.3 has no model-download path; the suite must fail if one appears. It does not replace keyboard, screen-reader, clean-install, Gatekeeper, SmartScreen, signing, audio-output, or release-asset testing.

## Platform and privacy gates

The official `tauri-driver` does not drive macOS WKWebView. Before enabling macOS automation, create a separate test-only Tauri build that compiles and registers an embedded driver only for that artifact, then prove through a release-contract check that the plugin and capability are absent from application and installer builds.

Use only synthetic, non-confidential inputs. Never capture or commit filenames, paths, case names, transcript content, audio metadata, byte samples, hashes, or record-derived values. Run a future harness in an isolated user-data directory and prove that it leaves no app, driver, or media processes behind.

Before making a release-critical subset blocking, demonstrate repeatable success on clean Windows and Linux runners, characterize retries and false failures, and verify that the same immutable artifact under review is the one exercised.

## Static client-size budget

The dependency-free client-size check guards the uncompressed JavaScript and CSS emitted into `dist/`:

```text
npm run build
npm run size:check
```

The current budgets are 560 KiB of JavaScript and 64 KiB of CSS. They leave modest headroom above the August 24, 2026 baseline of 487.6 KiB of JavaScript and 52.9 KiB of CSS while still surfacing material growth. The script walks `dist/` deterministically, includes all `.js`, `.mjs`, and `.css` assets, excludes source maps and other asset types, and fails when the build output is absent.

This is an engineering regression guard, not a claim about installer size, download transfer, runtime memory, startup time, or user-perceived performance. It runs as a blocking CI step immediately after the frontend build. Budget changes require an intentional review of the emitted assets and the reason for the increase.
