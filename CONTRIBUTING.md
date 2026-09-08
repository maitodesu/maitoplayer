# Contributing

Use Rust 1.96.0 and pnpm 11.24.0. Keep privileged operations behind typed
backend commands and opaque IDs. Frontend code must never accept an arbitrary
filesystem path, invoke a process, or treat subtitle/dictionary text as HTML.

Before submitting a change, run the workspace tests, clippy, frontend checks,
and `git diff --check`. Generated bindings, lockfiles, shared contracts, root
configuration, and composition files require integrator review.

Do not commit source media, generated dictionary databases, caches, credentials,
user paths, or diagnostic bundles.

