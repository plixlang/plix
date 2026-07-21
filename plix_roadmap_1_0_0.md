# Plix v1.0.0 — Final & Stable Roadmap

> **Status:** Planned  
> **Target channel:** Stable / Long-Term Support (LTS)  
> **Prerequisite release:** v0.9.5 (release hardening, documentation, versioning, test/CI baseline)

Plix v1.0.0 is the first stable, production-oriented release of the Plix
language, toolchain, runtime, package ecosystem, and official distribution
artifacts. This document defines the scope and measurable release gates; a
feature is not considered complete merely because its implementation exists.

## 1. v1.0.0 contract

### 1.1 Stability and API freeze

Before the v1.0.0 release candidate (RC) phase:

- The language grammar, core syntax, standard-library public API, CLI commands,
  package manifest format, and public FFI API must be versioned and documented.
- A public API inventory must be published, including each supported target and
  its compatibility status.
- New breaking changes require an approved language-change record before the RC
  phase; once RC begins, they are deferred to v2.0.0 unless they fix a critical
  security defect.

### 1.2 Backward compatibility

v1.x follows semantic versioning:

- A valid v1.0.0 program and package remains supported throughout v1.x unless a
  security fix makes the previous behavior unsafe.
- Additions are backward compatible; removals and incompatible behavior changes
  are reserved for a new major version.
- Deprecations include a diagnostic, migration guidance, and at least one minor
  release of notice before removal in the next major release.
- Compatibility is enforced with a checked-in corpus of language, CLI, package,
  and FFI fixtures in CI.

### 1.3 LTS policy

The exact support calendar will be announced at release time. The proposed
policy is:

| Stream | Intended support | Scope |
|---|---:|---|
| v1.0 LTS | 24 months from general availability | Security fixes, critical correctness fixes, supported-platform build fixes |
| Current stable v1.x | Active development | Features, improvements, normal bug fixes |

LTS fixes are backported from the current stable branch when practical and are
released as patch versions. Every security fix receives an advisory and clear
upgrade guidance.

## 2. Performance finalization

Performance claims must be reproducible and scoped. Results must include Plix
version/commit, compiler flags, operating system, CPU, benchmark source,
workload, methodology, and raw results.

### 2.1 Interpreter target

**Goal:** on the published representative benchmark suite, Plix interpreter
throughput is at least **on par with CPython** on representative workloads.

Acceptance requirements:

- Compare against a pinned CPython version on identical hardware and workloads.
- Use warm-up, multiple runs, median and variance reporting.
- Include both numeric and allocation/string/control-flow workloads; do not
  advertise a single microbenchmark as a general result.
- Preserve interpreter/native observable-output parity.

### 2.2 Native target

**Goal:** Plix native output approaches C++ performance on the explicitly published
benchmark subset.

This is deliberately scoped: "approaches C++" is not a universal guarantee.
The release claim must name the C++ compiler/version, flags, source, hardware,
and benchmark subset. C++ baselines should use an optimized production build
(e.g. `-O3` or equivalent) and equivalent algorithms.

### 2.3 Performance regression gates

- A versioned benchmark harness runs in CI or scheduled performance CI.
- Baseline data is retained by release/commit.
- Material regressions require an issue, owner, and explicit release decision.
- Performance-sensitive changes include a benchmark before/after report.

## 3. Memory and reliability

**Goal:** zero known memory leaks in supported workloads and runtime paths.

“Zero leaks verified” is a test claim, not an absolute proof. v1.0.0 requires:

- Leak detection under representative interpreter and native workloads.
- Automated AddressSanitizer/LeakSanitizer (or platform-equivalent) jobs where
  supported.
- Long-running stress tests for allocation, ARC, arenas, closures, arrays/maps,
  structs, errors, networking, and Python FFI boundaries.
- Fuzzing and differential interpreter/native tests, with minimized regression
  inputs checked into the repository.
- Explicit documentation of known limitations, unsupported targets, and any
  intentionally process-lifetime allocations.

## 4. Security finalization

### 4.1 Sandboxed execution

**Goal:** a documented and independently tested sandbox mode for untrusted
Plix code.

The threat model must state what the sandbox does and does not protect against.
At minimum it must define controls for filesystem access, network access,
process execution, environment variables, resource limits, module/package
resolution, native/FFI access, and platform-specific isolation behavior.

The term **“certified”** may only be used after the certifying organization,
scope, platform coverage, version, and report are public. Until then, release
language must say “sandbox tested” or “sandbox hardened,” not “certified.”

### 4.2 Cryptography audit

**Goal:** independent audit of Plix cryptographic code and crypto-relevant
integration paths.

Release requirements:

- Never present an internal review as an independent cryptographic audit.
- Publish the audit scope, audited revision, findings, remediation status, and
  residual risks where disclosure is allowed.
- Use well-maintained cryptographic primitives/libraries rather than bespoke
  algorithms unless an exceptional, peer-reviewed reason exists.

### 4.3 Secure module system

The module and package system must define:

- package identity, immutable versions, lockfiles, dependency resolution, and
  reproducible installation;
- registry transport security and integrity verification;
- package signing/provenance strategy and key rotation/revocation process;
- permission/capability model for packages where applicable;
- protection against dependency confusion, typosquatting, path traversal, and
  unsafe install-time execution;
- vulnerability reporting and incident-response process.

## 5. Ecosystem deliverables

### 5.1 Package manager — `plix pm`

Deliver a stable package manager command group with documented behavior for:

```text
plix pm init
plix pm add <package>
plix pm remove <package>
plix pm install
plix pm update
plix pm publish
plix pm search <query>
```

v1 requirements include a manifest, lockfile, deterministic resolution,
workspace/project support as designed, offline/cache behavior, clear errors,
and integration tests for dependency graphs and failure cases.

### 5.2 Official registry

The official registry must have availability, moderation, ownership transfer,
yank/deprecation, provenance, package retention, abuse reporting, backup, and
incident-response policies. A documented local/private-registry workflow is
also required for enterprise and offline users.

### 5.3 Editor tooling

Officially supported editor integrations:

| Editor | Minimum v1 capability |
|---|---|
| VS Code | Installable extension; syntax highlighting, diagnostics, formatting, go-to definition, completion where supported by the language server |
| Vim/Neovim | Supported configuration/plugin and documented LSP integration |
| Emacs | Supported configuration/package and documented LSP integration |

All plugins must pin or declare compatible Plix/LSP versions and include a
release/test process.

### 5.4 Documentation

The documentation site/repository must include versioned, reviewed guides for:

- installation and upgrades on every supported platform;
- language grammar and a tutorial/reference split;
- standard library API reference with examples;
- type system, ownership, memory model, concurrency/async model, and errors;
- native compilation, WebAssembly, debugging, formatting, linting, testing,
  package management, and performance profiling;
- Python FFI and any other supported FFI, including lifetime/threading and
  safety constraints;
- sandbox/security model and supply-chain policy;
- migration guide from pre-v1 releases; and
- contributor, governance, code-of-conduct, security-reporting, and release
  documentation.

## 6. Official distribution

v1.0.0 must publish signed/checksummed, reproducible where feasible, release
artifacts for the documented supported target matrix:

- Linux x86_64;
- Windows x86_64;
- macOS Intel (x86_64);
- macOS Apple Silicon (aarch64);
- additional targets only after they have the same install/test/support story.

Each release includes checksums, signatures/provenance when available, release
notes, upgrade notes, a support matrix, and smoke-test evidence.

### 6.1 Docker image

Publish an official minimal Docker image with:

- immutable version tag and content digest;
- documented base image and supported architectures;
- non-root default execution where practical;
- SBOM and vulnerability-scanning policy; and
- examples for compilation, execution, and sandbox-aware deployment.

### 6.2 WebAssembly

Publish an official WebAssembly build only after its supported feature set is
explicitly documented. The target must state its runtime host requirements,
WASI/browser compatibility, I/O/network/FFI constraints, build command,
debugging guidance, and parity test coverage.

## 7. Release gates

v1.0.0 can enter RC only when all feature work is complete and the following
are green:

1. API/grammar/CLI/package format freeze and compatibility corpus pass.
2. Full unit, integration, end-to-end, negative, fuzz, and interpreter/native
   parity suites pass on the supported platform matrix.
3. Memory-safety and stress-test evidence is reviewed.
4. Security threat model, sandbox tests, dependency-supply-chain controls, and
   audit status are published.
5. Performance report meets the scoped targets and has reproducible sources.
6. Documentation is complete, versioned, and reviewed.
7. Package manager/registry and editor integration acceptance suites pass.
8. Official binary, Docker, and (if in scope) WebAssembly artifacts are built,
   verified, and install-smoke-tested.
9. Release candidate completes a defined stabilization period with no unresolved
   release-blocking defects.
10. Maintainers approve the final release checklist and publish known issues.

## 8. Suggested milestones

| Milestone | Focus | Exit condition |
|---|---|---|
| v0.9.5 | Release hardening baseline | Versioning/doc cleanup, CI baseline, test inventory, initial Rust unit tests, changelog and v1 design documents |
| v0.9.6–v0.9.x | Core stabilization | Compatibility corpus, runtime safety work, benchmark infrastructure, security threat model |
| v0.10.x | Ecosystem preview | Package manager/manifest/lockfile and registry preview; early editor/LSP packages |
| v1.0.0-beta | Feature complete | Public beta, migration guide, packaging and platform test matrix complete |
| v1.0.0-rc | API freeze | Only release-blocking security/correctness/performance fixes allowed |
| v1.0.0 | Stable LTS | All release gates above satisfied and artifacts published |

## 9. Definition of done

The v1.0.0 release is complete only when the stated deliverables are publicly
usable, documented, tested, reproducible where claimed, and supported under
the published LTS policy. Marketing claims about speed, leak freedom,
certification, or audit status must always link to the underlying scope and
evidence.
