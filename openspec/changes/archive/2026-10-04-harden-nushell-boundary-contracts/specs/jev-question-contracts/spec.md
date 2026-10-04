# Spec Delta

## MODIFIED Requirements

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

Raw question maps SHALL follow the TypeSafe `0.2.0` field shapes. Instructions SHALL accept omission, string, object, array, or null. Noul criteria SHALL accept omission, null, or a record whose `true` and `false` descriptions accept string/object/array/null. Choice SHALL require a criteria record with descriptions of those same root types. Score SHALL require a nonempty list of string/object/array level descriptions; root null SHALL be rejected. Structured descriptions SHALL permit ordinary nested JSON scalars. Missing optional fields and explicit null fields SHALL remain distinguishable. Raw maps SHALL NOT receive constructor-only cardinality maxima. Unknown fields in a raw question record SHALL be rejected rather than ignored; duplicate keys in nested records selected for that question SHALL follow the lossless outbound conversion rule.

#### Scenario: Structured instructions

- **WHEN** a question's instructions are a record containing text and nested scalar data
- **THEN** the instructions are preserved as a structured object

#### Scenario: Omitted instructions and explicit null

- **WHEN** one raw Noul question omits instructions and another supplies null
- **THEN** the request preserves absence for the first and an explicit null for the second

#### Scenario: Scalar instruction or null Score level

- **WHEN** instructions are an integer or a Score level is null
- **THEN** validation fails and identifies the offending question field

#### Scenario: Unknown raw question field

- **WHEN** a raw Noul, Choice, or Score question includes a misspelled or unsupported top-level field such as `instrucitons`
- **THEN** validation fails before HTTP dispatch instead of dropping that field

#### Scenario: Duplicate nested question key

- **WHEN** a selected raw question contains two Nu record fields with the same nested key
- **THEN** conversion fails with the location of that key before HTTP dispatch

### Requirement: Offline constructors return ordinary data

`jev question noul`, `jev question choice`, and `jev question score` SHALL return ordinary Nu records in request-question format. Constructors SHALL accept instructions using an `any` argument shape followed by question-field validation. They SHALL work without configuration or credentials and SHALL NOT perform HTTP requests or execute Nu closures. Their results SHALL be usable in records loaded from NUON or returned from Nu modules. Nonempty pipeline input SHALL be rejected rather than silently ignored; instructions SHALL be supplied as arguments.

#### Scenario: Assemble a policy from constructors

- **WHEN** constructor results are assigned to named fields in a questions record
- **THEN** the record can be passed directly to `jev ask` or `jev annotate`

#### Scenario: Constructor is independent of the network

- **WHEN** the service is unavailable and no key is configured
- **THEN** a valid constructor still returns its question record

#### Scenario: Constructor receives pipeline input

- **WHEN** a nonempty value is piped to any of the three question constructors
- **THEN** the invocation fails instead of ignoring the piped value
