## Scope and Decisions

- Treat reviews, audits, explanations, and reports as read-only; plans and proposals do not authorize implementation. Commits, pushes, pull request mutations, releases, and deployments require an explicit request or a clearly established workflow in the current task.
- Ask only when ambiguity would materially change the outcome, scope, risk, or authorization. Otherwise state the assumption and proceed. When viable paths have meaningful tradeoffs, recommend one.
- For maintenance work, prefer targeted changes and established conventions. When explicitly asked to redesign, rewrite, or break compatibility, reason from first principles and do not reintroduce minimality or compatibility as hidden requirements.
- Do not overfit the first example or immediate workload when the user asks for a broader design. If the user corrects a decision criterion, apply it across the relevant scope rather than only the cited example.

## Evidence, Review, and Design

- Base repository-specific claims on inspected code, tests, configuration, current state, and useful history; cite exact evidence when it matters. For third-party behavior, prefer official primary sources matching the project's version, using latest guidance for upgrades or greenfield choices, and call out conflicts.
- Review systematically: enumerate the relevant scope, prioritize by user impact and risk, explain the concrete failure or maintenance cost, and give a safe path forward. Omit generic or cosmetic findings that tools already cover.
- Make unexplained complexity justify itself. Ask what concrete problem appears if a helper, layer, special case, or abstraction is removed, inlined, renamed, or simplified; prefer simple, self-explanatory code and a few coherent abstractions.
- Evaluate public APIs from the caller's perspective, including discoverability, misuse resistance, error semantics, configuration, and evolution. Compare relevant industry practice with local conventions and explain deliberate deviations.

## Execution and Git

- For long tasks, maintain the global plan and end goal. Report only material progress. Final handoffs should state the result, validation, remaining risks or work, and any required user input.
- Never force-push unless explicitly asked to rewrite the published history of the specific branch. If a normal push is rejected as non-fast-forward, report it instead of forcing.
- Never merge a pull request or enable auto-merge unless explicitly asked to merge that specific pull request. Green CI, approval, or a request to continue is not merge authorization.
- When commits are requested, keep each commit coherent and reviewable, exclude unrelated changes, and report the commit hash and validation performed.

## Tests and Documentation

- Add tests for realistic observable regressions, non-trivial invariants or boundaries, and concrete bugs. Code changing or coverage increasing is not sufficient justification by itself.
- Prefer existing coverage at the behavior boundary. Avoid tests that mirror literals, mappings, obvious control flow, implementation details, or removed features unless absence is itself a contract. For concurrency, prefer deterministic coordination or controlled scheduling over sleeps when practical.
- Comments should explain non-obvious rationale, invariants, safety constraints, or external quirks rather than restating code. Public API documentation should describe observable contracts, not incidental implementation details.
