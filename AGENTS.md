# Agent Instructions

## Scope and priorities

- Follow the user's task and applicable higher-priority instructions. Ask if requirements conflict or are materially ambiguous.
- Make precise, task-focused changes. Preserve unrelated work and avoid unrelated edits.
- Prefer simple, idiomatic solutions that fit the existing codebase.

## Project

- This is a Rust 2024 terminal editor. The current supported platform is macOS; do not claim support for other platforms without evidence.
- The project requires stable Rust 1.89 or newer.
- Preserve the manifest's `unsafe_code = "deny"` and Clippy `all = "deny"` lint settings.
- Avoid incidental dependency changes. Prefer stable, maintained dependencies; explain the need and ask before adding a dependency.

## Workflow

- For small, clear tasks, proceed directly. For broad, risky, or ambiguous work, summarize the approach and seek approval before implementation.
- Ask focused questions when a decision would materially affect behavior or scope.
- Update directly related documentation when behavior, configuration, or usage changes.
- Keep errors actionable and visible; do not silently swallow failures or return success-shaped fallbacks.

## Implementation and tests

- Follow existing architecture, naming, formatting, and error-handling patterns. Avoid unnecessary abstractions and defensive checks for impossible states.
- Validate only inputs that affect correctness or safety. Use safe defaults and bounded I/O or concurrency where relevant.
- Add focused tests for behavior introduced or fixed, including important failure cases when practical. Prefer existing test patterns.
- Keep comments short and explain why when the code is not self-explanatory.

## Validation

- Run the smallest relevant checks for the change. Common Rust checks are `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets`.
- Documentation-only changes generally do not need build or test runs.
- Report the checks run and any that could not be completed.

## Responses

- Give a concise summary suited to the task. Use code blocks when they improve readability, not as a requirement for every response.
