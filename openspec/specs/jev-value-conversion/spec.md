# jev-value-conversion Specification

## Purpose

Preserve structured Nu data when building Jev states and expose JSON responses as ordinary Nu values with explicit rules for unsupported or lossy conversions.

## Requirements

### Requirement: Recursive structured conversion

The plugin SHALL convert Nu strings, integers, finite floats, booleans, nothing, lists, and records recursively to corresponding JSON strings, numbers, booleans, null, arrays, and objects. Records and lists SHALL NOT be serialized to text or NUON. JSON-looking strings SHALL remain strings.

#### Scenario: Nested record state

- **WHEN** input contains a record with nested lists, numbers, booleans, and null values
- **THEN** the request state preserves those shapes and values as JSON data

#### Scenario: String containing JSON syntax

- **WHEN** a string value contains `{"message":"hello"}`
- **THEN** its request state is a JSON string rather than a parsed object

### Requirement: Explicit special-value representation

Nu dates SHALL convert to RFC3339 strings, durations SHALL convert to exact signed nanosecond strings ending in `ns`, and file sizes SHALL convert to integer byte counts. These representations SHALL be documented and consistent in live requests and dry runs.

#### Scenario: Special values in a record

- **WHEN** a state record contains a date, a one-second duration, and a 1024-byte file size
- **THEN** its JSON fields contain the RFC3339 date, `1000000000ns`, and integer `1024`, respectively

### Requirement: Unsupported values fail explicitly

Binary, closure, range, custom, cell-path-as-data, non-finite float, and other unsupported values SHALL produce an error identifying the offending value location. The plugin SHALL NOT replace unsupported values with debug strings. An incoming Nu error SHALL remain an error.

#### Scenario: Nested binary value

- **WHEN** the selected state includes a binary value at `payload.bytes`
- **THEN** conversion fails with a location-aware state error before sending a request

#### Scenario: Upstream error

- **WHEN** an upstream Nu error is encountered while reading the state
- **THEN** it is propagated rather than submitted as an ordinary JSON value

### Requirement: Final state admissibility

The final API state SHALL be a string, object, or array. Nested JSON scalars SHALL remain valid. Empty pipeline input and undecoded ByteStream input SHALL be rejected by `jev ask`; an explicitly supplied empty list SHALL remain a valid array.

#### Scenario: Unsupported top-level scalar

- **WHEN** integer input is supplied without context wrapping
- **THEN** the command rejects the state before HTTP dispatch

#### Scenario: Empty list as one state

- **WHEN** an explicitly supplied empty list is evaluated
- **THEN** the request state is `[]`

### Requirement: Explicit additional context wrapping

For table commands, whole-row selection, cell-path selection, or explicit field projection SHALL occur before recursive conversion and context wrapping. Only the chosen outbound input and explicit context SHALL be converted; unrelated source fields SHALL NOT be converted or transmitted. When `--context` is absent, the converted chosen input SHALL be the state. When `--context` is present, the state SHALL be exactly `{input: <converted input>, context: <converted context>}`. Flag presence SHALL be distinguished from explicit null. The plugin SHALL NOT merge context into the input or overwrite its fields. Final state admissibility SHALL be checked after wrapping.

#### Scenario: Input has a field named context

- **WHEN** an input record already contains `context` and additional context is supplied
- **THEN** the complete original record is nested under `input`
- **AND** the supplied context appears separately under the outer `context` field

#### Scenario: Explicit null context

- **WHEN** integer input is supplied with `--context null`
- **THEN** the final state is an object containing the integer under `input` and null under `context`

#### Scenario: Projected values are converted without unrelated source fields

- **WHEN** a row contains a date in `sent_at` and unrelated binary data, and `--fields [sent_at] --context null` is supplied
- **THEN** the state is exactly `{input: {sent_at: <RFC3339 date>}, context: null}`
- **AND** the unrelated binary data does not cause conversion failure or appear in the request

### Requirement: Lossless response conversion

JSON responses SHALL convert recursively to ordinary Nu values. JSON strings SHALL remain strings, and numeric values SHALL NOT be silently rounded to fit Nu integers. An integer outside Nu's supported integer domain SHALL cause a response conversion error.

#### Scenario: Structured probabilities

- **WHEN** an API answer contains nested probability and legend objects
- **THEN** those objects are accessible as ordinary Nu record fields without parsing a string

#### Scenario: Oversized response integer

- **WHEN** a response contains an integer that cannot be represented losslessly by Nu
- **THEN** the command returns a response error rather than a rounded numeric value
