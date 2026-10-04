# Spec Delta

## ADDED Requirements

### Requirement: Bounded successful response bodies

The plugin SHALL accept at most 16 MiB (16,777,216 bytes) of body data from a successful System One or model-list response. It SHALL reject an oversized declared `Content-Length` before reading the body and SHALL also enforce the limit while reading a response without a usable length declaration. An oversized body SHALL produce a nonretryable response error without JSON decoding, logging its contents, or returning partial data. The limit SHALL apply to each successful response separately, not to the sum of retries or rows.

#### Scenario: Declared oversized body

- **WHEN** a successful response declares a body larger than 16 MiB
- **THEN** the plugin rejects it without reading or decoding the body

#### Scenario: Chunked oversized body

- **WHEN** a successful chunked response has no usable length declaration and exceeds 16 MiB while being read
- **THEN** the plugin stops reading and returns a response error without retrying it

#### Scenario: Body at the limit

- **WHEN** a successful body has exactly 16 MiB of data and satisfies the endpoint's JSON contract
- **THEN** the size limit alone does not reject it
