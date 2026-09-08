# Security policy

## Reporting a vulnerability

Report suspected vulnerabilities privately to the maintainers before public
disclosure. Include the application version, operating-system build, concise
reproduction steps, impact, and whether the issue works with the default Tauri
capability file. Do not attach user media, Anki collections, full local paths,
environment dumps, or unredacted diagnostics. A minimal synthetic fixture is
preferred.

No public security contact has been configured for this pre-release repository.
Release publication is blocked until the project owner records a monitored
private contact and a supported-version policy.

## Security boundary

The desktop security boundary is intentional:

- The webview receives backend-issued media URLs and opaque IDs, not arbitrary
  path, filesystem, shell, or process commands.
- Session media responses are range-capable and capped at 8 MiB per response.
- External media tools are launched directly with argument arrays, timeouts, and
  bounded output; no command shell is involved.
- The user-selected source remains canonical. Temporary conversion output is not
  exposed until promotion.
- The CSP allows no remote script, object, or frame content. The window capability
  grants `core:default` only.
- Subtitle and dictionary content is rendered as text. Anki fields are escaped
  before application-generated markup is added.
- Product processing is local; telemetry and remote media upload are out of
  scope. Confirmed card assets are sent only to AnkiConnect on loopback.

Run `tests/security/verify-desktop-boundary.ps1` for the deterministic static
checks. Passing it does not replace malicious-input testing, packaged effective
ACL inspection, child-process cleanup testing, or clean-machine verification.
