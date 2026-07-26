# Domain Docs

How engineering skills consume this repository's domain documentation.

## Before exploring

- Read the root `CONTEXT.md` when it exists.
- Read root ADRs under `docs/adr/` that affect the area being changed.
- If these files do not exist, proceed silently; create them only when a glossary term or durable architectural decision needs to be recorded.

## Layout

This repository uses the single-context layout:

```text
/
├── CONTEXT.md
├── docs/adr/
└── src/
```

## Consumer rules

Use terms defined by `CONTEXT.md` consistently in issues, specifications, implementation plans, test names, and documentation. Explicitly surface any conflict with an existing ADR instead of silently overriding it.
