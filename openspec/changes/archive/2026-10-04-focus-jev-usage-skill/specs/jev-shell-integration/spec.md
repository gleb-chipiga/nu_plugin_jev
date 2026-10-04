# Spec Delta

## ADDED Requirements

### Requirement: Task-focused repository usage skill

The repository-owned usage skill SHALL include implemented guidance only when it affects command or flag choice, outbound data, result or error interpretation, access configuration, or material disclosure risks, excluding internal mechanics with no such effect.

#### Scenario: Internal cancellation mechanism

- **WHEN** an engineering spec describes what happens to an external iterator blocked in `next()` during cancellation
- **THEN** the usage skill omits that mechanism because it does not change how an agent composes or consumes a Jev pipeline

#### Scenario: Actionable outbound-state selection

- **WHEN** `jev annotate` offers options that change which source fields are sent to the API
- **THEN** the usage skill explains those options because they change what data leaves Nushell
