# Skills Generation Information

This document records how the paperreader skills were generated and how to keep them synchronized with the CLI tool's evolving feature set.

## Generation Details

**Generated from source at:**

- **Commit SHA**: `2dca0244aeb0dd5e6d7f0b6d01eb4973ef614e25`
- **Date**: `2026-06-22`
- **Commit**: `fix: index link broken, --toc quality overhaul`

**Source documentation:**

- Project README: `README.md`
- CLI help: `paperreader --help` (generated from `src/cli.rs` doc comments)
- CLAUDE.md: not present at project root

**Generation date**: `2026-06-22`

## Structure

```
skills/
├── GENERATION.md              ← this file
└── paperreader/
    └── SKILL.md                ← model-invoked skill wrapping the paperreader CLI
```

Single skill with no disclosed reference files. All reference (commands, JSON schema, diagnostics) is inline in SKILL.md.

## File Naming Convention

No established prefix patterns — the skill consists of a single `SKILL.md` with no `references/` directory. If reference files are added in the future, consider prefixing by domain (e.g. `schema-*.md` for JSON schemas, `cmd-*.md` for command reference).

## Reference Files

None. The paperreader skill keeps all reference inline:

| Content | Location | Rationale |
|---------|----------|-----------|
| Command reference | `SKILL.md` §Index & search, §List & inspect | Every branch needs every command |
| JSON output schema | `SKILL.md` §JSON output | Primary output format, needed by search branch |
| Deep reading rule | `SKILL.md` §Deep reading | Core behavioral anchor, must always be in context |
| TOC hierarchy levels | `SKILL.md` §List & inspect | Co-located with the `--toc` command entry |

## How to Update Skills

When the paperreader CLI gains new flags, changes output format, or changes behavior:

### 1. Check for Changes

```bash
# Diff CLI help text
git diff <last-sha>..HEAD -- src/cli.rs

# Diff main search/output logic
git diff <last-sha>..HEAD -- src/main.rs src/search.rs src/index.rs

# Check for new modules
git diff --name-status <last-sha>..HEAD -- src/
```

### 2. Update Process

**For new CLI flags:**
- Add the command to the relevant section in `SKILL.md`
- If it enables a new workflow, add a dedicated section or a prose line describing when to use it
- Update this file's reference list

**For output format changes:**
- Update the JSON schema block in `SKILL.md` §JSON output
- Update any prose that references affected fields

**For behavioral changes (indexing, search strategy):**
- Update the prose under the affected section
- If a diagnostic or warning changes, update the corresponding guidance

### 3. Update Checklist

- [ ] Run `paperreader --help` and diff against the current commands in SKILL.md
- [ ] Test new flags with `--json` and verify the schema matches the documented one
- [ ] Update `SKILL.md` with new commands, output, or behavior
- [ ] Update this `GENERATION.md` with new SHA and date

## Style Guidelines

- Practical, actionable guidance over exhaustive reference
- Concise code examples with inline comments
- Every section anchored by a **leading word** ("Deep reading", "never guess")
- Co-locate rules with the commands that implement them
- Keep implementation details (detection heuristics, page sampling strategy) out of the skill

## Version History

| Date       | SHA      | Changes |
| ---------- | -------- | ------- |
| 2026-06-22 | `2dca024` | Initial generation. TOC two-phase detection, index link fix, natural hierarchy levels, code-snippet filter. |

---

Last updated: 2026-06-22
Current SHA: 2dca024
