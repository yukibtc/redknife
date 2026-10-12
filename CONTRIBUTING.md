# Contributing

Before writing code, read [CODE_STYLE.md](CODE_STYLE.md).

## Contribution workflow

Fork the repository, create a topic branch, and submit the change as a pull request or patch.

Keep changes focused and avoid unrelated refactors. Add or update tests when behavior changes, and update documentation when public behavior or APIs change.

Keep commits atomic and understandable. Preserve public API compatibility and avoid new dependencies unless required.

Add notable user-facing changes to the appropriate unreleased section of `CHANGELOG.md`, following the existing style.

## Simplicity and design

Prefer the simplest implementation that correctly satisfies the current requirements.

Keep abstractions, indirection, configuration, and architectural layers proportionate to the problem.
Avoid speculative generalization or extensibility that is not justified by a concrete requirement, public API constraint, or established extension point.

Some problems are inherently complex; their implementations should still be made as simple,
local, and explicit as reasonably possible. Any added complexity should be justified by a concrete
current requirement.

When multiple correct approaches exist, prefer the one with fewer moving parts and lower
maintenance cost.

## Comments

Keep straightforward code self-explanatory.
Add inline implementation comments where the reasoning is not obvious from the code itself.
In particular, document important assumptions, invariants, ordering constraints, state transitions,
edge cases, non-obvious control flow, and implementation choices that might otherwise be misunderstood or accidentally changed.

## Coding conventions

Install [just](https://github.com/casey/just) and run `just precommit` to format and check the code
before committing. CI enforces the same checks.
