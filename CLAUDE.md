# CLAUDE.md

## Agent skills

Matt Pocock's skills (MIT, pinned copy in `.claude/skills/`, see `docs/agents/mattpocock-skills.md`). Use them in the order below for every change.

### Workflow order

1. `/grill-with-docs`: interview before the change; settle terms in `GLOSSARY.md` and record decisions.
2. `/to-spec`: turn the agreed plan into a spec issue.
3. `/to-tickets`: break the spec into blocking-linked issues.
4. `/implement`: build each ticket with `tdd` at the agreed seams.
5. `/code-review`: review the diff before every pull request (Claude Code's built-in skill).
6. `/retro`: after the session, suggest fixes to this repo's agent setup.

For a bug, start with `diagnosing-bugs` in place of steps 1 to 3. For work too big for one session, run `/wayfinder` before `/to-spec`. Use `/handoff` when another agent will pick the work up.

### Issue tracker

GitHub Issues on this repo, via `gh`. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context. Decisions go in `DECISIONS.md`, not `docs/adr/`. See `docs/agents/domain.md`.
