# Changelog

All notable user-facing changes are documented here. Plix follows
[Semantic Versioning](https://semver.org/) after v1.0.0. Until then, releases
may make compatible or incompatible changes; migration notes will identify
any known breakage.

## [Unreleased]

No unreleased changes are recorded yet.

## [0.9.5] - 2026-07-21

### Changed

- Unified the toolchain and embedded-runtime package versions at `0.9.5`; the
  CLI now derives its version from Cargo package metadata to avoid a third,
  drifting version constant.
- Reworked the top-level documentation, installation, tooling, testing,
  security, contribution, and release guidance.
- Added a public v1.0.0 stable/LTS roadmap with measurable release gates.
- Expanded the GitHub Actions quality and packaging workflow.

### Added

- Rust unit coverage for lexer, parser, and project-manifest behavior.
- Release preflight verification for version consistency and the release test
  battery.

### Fixed

- Ignored local packaging and parity-test artifacts so they are not accidentally
  committed.

## [0.9.0]

- Historical development release. Consult the Git history for the complete
  pre-0.9.5 change set.
