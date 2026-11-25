# Changelog fragments

Write each change's release notes in its own `changes/<slug>.md` file. Name
it after the branch without its prefix. For example, `slice/pane-focus` uses
`changes/pane-focus.md`. Choose a unique slug of lowercase letters, digits,
and hyphens. Keep `## [Unreleased]` in `CHANGELOG.md` empty.

Use one or more of `### Added`, `### Changed`, `### Deprecated`, `### Removed`,
`### Fixed`, and `### Security`. Give each heading at least one `- ` bullet.
Write entries for someone using Luna, as in `CHANGELOG.md`, and indent wrapped
lines by two spaces.

```markdown
### Fixed

- Luna remembers which pane was selected when the window closes.
```

When a change covered by the check needs no release note, use a single
`### None` heading followed by a non-empty line explaining why.

```markdown
### None

This change only adds coverage to an existing test module.
```

`./scripts/check-changes.ps1` checks the fragments. At release time,
`./scripts/release.ps1 -Bump <version>` groups the entries by heading and
removes the collected files, including exemptions.
