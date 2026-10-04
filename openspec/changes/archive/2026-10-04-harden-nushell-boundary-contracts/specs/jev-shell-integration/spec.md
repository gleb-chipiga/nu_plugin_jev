# Spec Delta

## ADDED Requirements

### Requirement: Truthful Nushell command input and output types

The registered Nu signatures SHALL declare `jev` as `nothing -> string`, `jev ask` as `any -> record`, `jev annotate` as `any -> list<any>`, `jev models` as `nothing -> record`, and each `jev question` constructor as `nothing -> record`. The broad `any` types SHALL preserve runtime validation of accepted Jev states and table rows; they SHALL NOT imply every value can be submitted to the service. The annotation output declaration SHALL allow unchanged non-record values under `--on-error keep`. The offline root command SHALL reject nonempty pipeline input instead of silently ignoring it.

#### Scenario: Command type discovery

- **WHEN** Nu inspects the seven registered command signatures
- **THEN** it reports the declared input and output types for each command
- **AND** `jev annotate` does not claim that every emitted item is a record

#### Scenario: Root guidance receives pipeline input

- **WHEN** a nonempty value is piped to bare `jev`
- **THEN** the invocation fails instead of discarding that value and returning guidance

#### Scenario: Annotation keeps a non-record row

- **WHEN** a non-record input row is handled under `jev annotate --on-error keep`
- **THEN** the unchanged row remains representable by the command's declared output type
