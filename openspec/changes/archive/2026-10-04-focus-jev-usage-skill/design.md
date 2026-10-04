# Design

## Context

The repository skill is loaded as task guidance for agents using the plugin. Its annotation section currently repeats blocked-iterator and local cancellation mechanics from the table-streaming specification. Those mechanics are valid engineering guarantees but do not affect the pipeline an agent writes.

## Goals / Non-Goals

**Goals:** Keep the skill short and oriented toward command choice, outbound data, results, and material safety decisions. Make the boundary durable for later skill updates.

**Non-goals:** Change cancellation, streaming, HTTP behavior, or the technical specifications that describe those guarantees. Rework the README or introduce another skill reference file.

## Decisions

- Remove the cancellation-mechanism sentence and any equally non-actionable implementation explanation found in the same focused review. Preserve guidance on state selection, response paths, errors, credentials, and diagnostic privacy because those affect use.
- Add a separate shell-integration requirement for the skill's content boundary. Keep the existing repository-skill requirement intact; it already governs existence and synchronization with user-visible behavior.
- Add the same short editorial criterion to `AGENTS.md`, where future skill maintainers will see it. An alternative was to rely only on the spec, but that would not guide routine edits that do not otherwise require reading OpenSpec.

## Risks / Trade-offs

- Over-pruning could hide a user-visible edge case. Review each removed sentence against whether it changes invocation, outbound disclosure, result interpretation, configuration, or response to failure.
- A textual scope rule is validated by review rather than a brittle string-matching test. Validate the skill structure and the OpenSpec change, and inspect the final diff.

## Migration Plan

No runtime or user-data migration is needed. The archived change and synchronized main spec become the maintained contract; reverting the documentation edit would restore only unnecessary guidance.
