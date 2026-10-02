# Spec Delta

## Purpose

Represent Jev policies as ordinary named Nu records with schema-aligned Noul, Choice, and Score questions and offline constructors for composing those records.

## ADDED Requirements

### Requirement: Named mixed question maps

Evaluation commands SHALL accept a nonempty record of named questions. Each question SHALL identify `noul`, `choice`, or `score` through its `type`. Multiple question types SHALL be accepted in the same map without splitting it into separate evaluations. Invalid maps SHALL fail before HTTP dispatch or table input consumption.

#### Scenario: Three question types in one map

- **WHEN** questions named `spam`, `kind`, and `urgency` have Noul, Choice, and Score types
- **THEN** the validated request preserves all three names and types in one `questions` object

#### Scenario: Empty map

- **WHEN** the supplied questions record is empty
- **THEN** the command returns a validation error without reading table rows or sending requests

### Requirement: Schema-aligned raw question fields

Raw question maps SHALL follow the TypeSafe `0.2.0` field shapes. Instructions SHALL accept omission, string, object, array, or null. Noul criteria SHALL accept omission, null, or a record whose `true` and `false` descriptions accept string/object/array/null. Choice SHALL require a criteria record with descriptions of those same root types. Score SHALL require a nonempty list of string/object/array level descriptions; root null SHALL be rejected. Structured descriptions SHALL permit ordinary nested JSON scalars. Missing optional fields and explicit null fields SHALL remain distinguishable. Raw maps SHALL NOT receive constructor-only cardinality maxima.

#### Scenario: Structured instructions

- **WHEN** a question's instructions are a record containing text and nested scalar data
- **THEN** the instructions are preserved as a structured object

#### Scenario: Omitted instructions and explicit null

- **WHEN** one raw Noul question omits instructions and another supplies null
- **THEN** the request preserves absence for the first and an explicit null for the second

#### Scenario: Scalar instruction or null Score level

- **WHEN** instructions are an integer or a Score level is null
- **THEN** validation fails and identifies the offending question field

### Requirement: Offline constructors return ordinary data

`jev question noul`, `jev question choice`, and `jev question score` SHALL return ordinary Nu records in request-question format. Constructors SHALL accept instructions using an `any` argument shape followed by question-field validation. They SHALL work without configuration or credentials and SHALL NOT perform HTTP requests or execute Nu closures. Their results SHALL be usable in records loaded from NUON or returned from Nu modules.

#### Scenario: Assemble a policy from constructors

- **WHEN** constructor results are assigned to named fields in a questions record
- **THEN** the record can be passed directly to `jev ask` or `jev annotate`

#### Scenario: Constructor is independent of the network

- **WHEN** the service is unavailable and no key is configured
- **THEN** a valid constructor still returns its question record

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
