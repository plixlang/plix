# Security model and reporting

## Current security status

Plix is pre-1.0 software. Its sandbox, package registry, package manager, and
supply-chain controls are roadmap items, not current security guarantees. Do
not run untrusted Plix programs with access to confidential files, credentials,
or production infrastructure solely on the basis of the current toolchain.

The optional Python FFI executes CPython extension/library code in-process.
Importing Python modules is therefore equivalent to trusting that Python
installation and its dependencies.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for a suspected vulnerability.
Follow the private reporting guidance in [`../SECURITY.md`](../SECURITY.md).
A report should include affected version/commit, operating system, a minimal
reproduction, impact, and any suggested mitigation.

## Security boundaries today

- Native compilation intentionally produces a host executable; it is not a
  sandbox.
- File system, networking, process, environment, and FFI access should be
  treated as host capabilities unless explicitly restricted by the deployment
  environment.
- Third-party `.px` modules should be reviewed before use. There is not yet an
  official package registry, signing system, or dependency lockfile.

## Path to v1.0.0

The [v1.0.0 roadmap](../plix_roadmap_1_0_0.md) defines the planned sandbox
threat model, module/package integrity requirements, and standards for external
security or cryptographic audit claims. Claims such as “certified sandbox” or
“cryptographically audited” must not be made until scope and evidence are
published.
