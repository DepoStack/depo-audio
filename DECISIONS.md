# Decisions

## 2026-11-24 Grill with docs (owner, docs only, nothing built)
- DepoAudio stays free. Converting, improving, playing and organizing need no account.
- DepoAudio stays a fully separate brand: no "by DepoStack" line on the app. depoaudio.com keeps one quiet made-by
  credit in its footer and nothing else (see the website repo's DECISIONS.md).
- DepoAudio does not connect to DepoStack yet: no depostack.com sign-in and no sending recordings to Transcript or
  Video. The owner wants to wait until DepoStack is closer to production and stable. Why it came up: the other
  products are moving to depostack.com accounts and link to each other (Exhibits, Transcript, Video), and an
  optional sign-in that sends recordings to Transcript or Video was considered. DepoAudio already has public users,
  its own site and a promise of no account and no uploads.
- When that time comes, the plan the owner sketched is: sign-in optional, and a recording stays on the computer
  until the person presses Send, with a clear note that it uploads to DepoStack. How the sign-in is named is still
  open, because it touches the separate-brand decision.
- The README's "No account or subscription is required" and "audio is not uploaded" stay true, and so do the same
  promises on depoaudio.com.
- People who want a recording in Transcript or Video move the file themselves for now.
- Merge: make it ready and ship it (owner, 2026-11-24). This replaces the 2026-11-13 decision to move it to a
  branch, which was never carried out. It closes the issue #90 readiness list first (M4A, 2 to 4 sources, a real
  per-source length, "Loudest source" in place of "Best Quality", tests) and the website shows it only with that
  release. Spec: issue #227. Tests sit at the merge job: two tones with a known offset in, the merged file and the
  offset out.
