# Shared engineering instructions

- Prefer simple, maintainable solutions and existing repository conventions.
- Preserve backward compatibility unless the request explicitly requires a change.
- Make small, focused, reviewable changes; do not modify unrelated files or overwrite unrelated work.
- Avoid unnecessary abstractions, dependencies, speculative implementations, and excessive logging or defensive checks.
- Use stable, actively maintained technologies and APIs. Verify dependency compatibility against official documentation before introducing or upgrading dependencies.
- Prefer dependencies with recent public releases and stable or LTS alternatives. Do not upgrade existing dependencies unnecessarily.
- Do not expose credentials, secrets, or sensitive information.
- Do not perform destructive operations, production deployments, or irreversible infrastructure changes without explicit authorization.
- Do not commit or push unless the user explicitly requests it.
- Run proportionate validation after implementation: formatting, builds, tests, linting, or type checks as applicable.
- Report actual validation results. Clearly distinguish passed, failed, and not-run checks.
- Findings must include severity, file and line location, evidence, impact, and a recommended correction.
- Never fabricate command execution, test results, review findings, or successful validation.
