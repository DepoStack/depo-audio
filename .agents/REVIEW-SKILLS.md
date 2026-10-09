# Project-local review skills

Prepared on the isolated `review/depo-audio-skills-and-audits` branch for a read-only review of Depo Audio at `1d0816e431d05eb5dcf04be6eb98bfd0efbb158f`.

## Authentic upstream packages

- Impeccable skill **4.5.1**: https://github.com/pbakaus/impeccable/tree/9dad388a41944a0d2b8d1fb547c8556d2ecb49e7/.agents/skills/impeccable . Full Codex skill subtree, including references, agents and scripts, is preserved unchanged. Upstream root Apache-2.0 LICENSE is included in the skill directory. The package tree is `787b90565852349a51eee01cccb3f6e3eb71ac90`, also identical at `1b6576e882258096eb0bb485482080169973b94b`.
- The Impeccable engine **0.1.12** and CLI package **4.1.0** are separate version tracks. No engine binary or CLI package is installed by this change.
- Addy Osmani agent-skills **0.6.12**: https://github.com/addyosmani/agent-skills/tree/1401c8b8030e023baeebb31781a6653fe8e93026 . Complete `skills/code-review-and-quality` plus all upstream shared `references/` are copied with their relative structure preserved under `.agents/`. Root MIT LICENSE and the upstream version manifest are included under `skills/code-review-and-quality/`.
- `SOURCE-MANIFEST.json` records original paths, immutable commits and Git blob hashes. Each copied upstream file was verified byte-for-byte against its Git blob SHA-1.

## Loading and scope

Manually load `skills/impeccable/SKILL.md` for design review and `skills/code-review-and-quality/SKILL.md` for code review. These are original upstream instructions; user authorization and read-only scope take precedence over any suggested edit, installation or publication flow.

No hooks, global configuration, application source, dependency manifests, lockfiles or environment image settings are changed. Agent configuration files are preserved as upstream support assets; they are not registered in a global or project runtime configuration.

The Impeccable launcher may fetch and execute a native engine and may write a user cache. For the read-only review, it was inspected but not executed, honoring the prohibition on untrusted binary execution and global environment changes. Context was read manually under the documented fallback. Deterministic detection, overlays and critique-storage engine commands remain unavailable unless an explicitly trusted engine is provisioned separately.

Filesystem presence and manual SKILL loading do not prove automatic skill discovery or UI picker registration. The initial executor skill catalog was empty. A fresh-task discovery check is still required; no cloud-wide setup correction or environment republish is claimed.
