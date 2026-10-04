# jev-question-contracts Specification

## Purpose

Represent Jev policies as ordinary named Nu records with schema-aligned Noul, Choice, and Score questions and offline constructors for composing those records.

## Requirements

### Requirement: Named mixed question maps

Evaluation commands SHALL accept a nonempty record of named questions. Each question SHALL identify `noul`, `choice`, or `score` through its `type`. Multiple question types SHALL be accepted in the same map without splitting it into separate evaluations. Invalid maps SHALL fail before HTTP dispatch or table input consumption. Repeated names in the outer Nu questions record SHALL be rejected rather than silently overwriting an earlier question.

#### Scenario: Three question types in one map

- **WHEN** questions named `spam`, `kind`, and `urgency` have Noul, Choice, and Score types
- **THEN** the validated request preserves all three names and types in one `questions` object

#### Scenario: Empty map

- **WHEN** the supplied questions record is empty
- **THEN** the command returns a validation error without reading table rows or sending requests

#### Scenario: Duplicate question name

- **WHEN** the supplied Nu questions record contains two fields with the same name
- **THEN** validation identifies the duplicate name before reading table rows or sending requests
- **AND** neither question silently replaces the other

### Requirement: Schema-aligned raw question fields

Raw questions SHALL follow TypeSafe `0.2.0`: instructions may be absent, string, object, array, or null. Noul criteria may be absent, null, or a record with `true`/`false` descriptions of those types. Choice SHALL require a criteria record of those types. Score SHALL require nonempty string/object/array levels and reject root null. Structured descriptions SHALL allow nested JSON scalars. Absence SHALL differ from explicit null; raw maps SHALL NOT inherit constructor-only cardinality maxima.

#### Scenario: Structured instructions

- **WHEN** a question's instructions are a record containing text and nested scalar data
- **THEN** the instructions are preserved as a structured object

#### Scenario: Omitted instructions and explicit null

- **WHEN** one raw Noul question omits instructions and another supplies null
- **THEN** the request preserves absence for the first and an explicit null for the second

#### Scenario: Scalar instruction or null Score level

- **WHEN** instructions are an integer or a Score level is null
- **THEN** validation fails and identifies the offending question field

### Requirement: Raw question fields retain lossless validation

Unknown raw question fields SHALL fail rather than be ignored. Duplicate keys in selected nested Nu records SHALL follow the lossless outbound conversion rule and fail with their location before HTTP dispatch.

#### Scenario: Unknown raw question field

- **WHEN** a raw Noul, Choice, or Score question includes a misspelled or unsupported top-level field such as `instrucitons`
- **THEN** validation fails before HTTP dispatch instead of dropping that field

#### Scenario: Duplicate nested question key

- **WHEN** a selected raw question contains two Nu record fields with the same nested key
- **THEN** conversion fails with the location of that key before HTTP dispatch

### Requirement: Offline constructors return ordinary data

`jev question noul`, `jev question choice`, and `jev question score` SHALL return ordinary Nu records in request-question format. They SHALL accept `any` instructions as arguments and validate question fields. They SHALL need no configuration or credentials, make no HTTP request, and execute no Nu closure. Results SHALL compose with NUON-loaded records or Nu modules. Nonempty pipeline input SHALL fail rather than be ignored.

#### Scenario: Assemble a policy from constructors

- **WHEN** constructor results are assigned to named fields in a questions record
- **THEN** the record can be passed directly to `jev ask` or `jev annotate`

#### Scenario: Constructor is independent of the network

- **WHEN** the service is unavailable and no key is configured
- **THEN** a valid constructor still returns its question record

#### Scenario: Constructor receives pipeline input

- **WHEN** a nonempty value is piped to any of the three question constructors
- **THEN** the invocation fails instead of ignoring the piped value

### Requirement: Noul constructor criteria

The Noul constructor SHALL emit `type: noul` and the supplied instructions. It SHALL accept `--yes` and `--no` descriptions using the allowed instruction root types. With neither flag present, it SHALL omit criteria. With either flag present, it SHALL emit only the explicitly supplied `true` and/or `false` criteria keys, including an explicitly supplied null.

#### Scenario: Only a yes description

- **WHEN** Noul construction supplies `--yes` and omits `--no`
- **THEN** criteria contains the `true` description and no implicitly inserted `false` description

### Requirement: Choice constructor shorthand and descriptions

The Choice constructor SHALL accept either a list of distinct strings or a criteria record. List strings SHALL become option names with null descriptions. It SHALL require between 1 and 255 option names, reject duplicate list names, and preserve structured or null record descriptions.

#### Scenario: List shorthand

- **WHEN** Choice construction receives `[normal promo spam]`
- **THEN** criteria is `{normal: null, promo: null, spam: null}`

#### Scenario: Duplicate option names

- **WHEN** Choice construction receives `[spam spam]`
- **THEN** it returns a validation error rather than collapsing two options

### Requirement: Score constructor levels

The Score constructor SHALL accept between 1 and 10 string/object/array descriptions and preserve their order. A level's list position SHALL correspond to score level `0..N-1`. Documentation SHALL recommend at least two levels and SHALL distinguish the constructor maximum from the raw schema's minimum-only cardinality rule.

#### Scenario: Ordered rubric

- **WHEN** Score construction receives `["later" "today" "now"]`
- **THEN** the request criteria retain that order, corresponding to levels 0, 1, and 2

#### Scenario: Constructor cardinality limit

- **WHEN** Score construction receives eleven levels
- **THEN** it returns a validation error before network access
