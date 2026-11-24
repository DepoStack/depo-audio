# Domain docs

How the engineering skills in `.claude/skills/` read and write this repo's domain documentation. Layout: single-context.

This repo follows the DepoStack convention, so two of the skills' defaults are mapped to files the repo already uses.

## Glossary

The glossary is `GLOSSARY.md` at the repo root. It may not exist yet: `/domain-modeling` creates it the first time a term is resolved. If it is missing, proceed silently.

When your output names a domain concept (an issue title, a refactor proposal, a hypothesis, a test name), use the glossary's term. Don't drift to synonyms it avoids. If the concept isn't there yet, either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Decisions (ADRs)

Decisions live in `DECISIONS.md` at the repo root, not in `docs/adr/`. Where the skills say "record an ADR" or "read the ADRs", use `DECISIONS.md`: read the entries that touch the area you're working in, and add a new dated entry in the style the file already uses (create the file if it doesn't exist yet). Do not create `docs/adr/`.

Offer a new entry only when the decision is hard to reverse, surprising without context, and the result of a real trade-off (see `.claude/skills/domain-modeling/ADR-FORMAT.md` for what an entry should cover).

If your output contradicts an existing entry, say so explicitly rather than silently overriding it.
