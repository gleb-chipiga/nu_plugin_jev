# Spec Delta

## MODIFIED Requirements

### Requirement: Recursive structured conversion

The plugin SHALL convert Nu strings, integers, finite floats, booleans, nothing, lists, and records recursively to corresponding JSON strings, numbers, booleans, null, arrays, and objects. Records and lists SHALL NOT be serialized to text or NUON. JSON-looking strings SHALL remain strings. A selected outbound Nu record containing duplicate keys at any depth SHALL fail conversion with the offending path before its request is submitted, rather than allowing JSON object construction to discard a value. Unselected source fields SHALL remain outside this validation boundary.

#### Scenario: Nested record state

- **WHEN** input contains a record with nested lists, numbers, booleans, and null values
- **THEN** the request state preserves those shapes and values as JSON data

#### Scenario: String containing JSON syntax

- **WHEN** a string value contains `{"message":"hello"}`
- **THEN** its request state is a JSON string rather than a parsed object

#### Scenario: Duplicate nested outbound key

- **WHEN** the selected state or explicit context contains a nested Nu record with two fields named `same`
- **THEN** the outbound conversion fails with a path identifying `same` before HTTP dispatch
- **AND** neither value is silently replaced in the request

#### Scenario: Duplicate key outside selected state

- **WHEN** a table row has duplicate keys only in an unselected field and `--state` selects another field
- **THEN** that unselected field is not converted or sent and does not invalidate the selected state
