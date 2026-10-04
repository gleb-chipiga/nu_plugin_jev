# Proposal

## Why

The row pipeline already bounds the number of in-flight operations, but a successful HTTP body is currently collected without a byte limit. Table requests also repeat local conversion and cloning of questions and static context for every row, even though those inputs are fixed for the invocation.

## What Changes

- **BREAKING for oversized responses:** Reject successful System One and model-list bodies above 16 MiB with a nonretryable response error. Enforce the bound before reading when `Content-Length` declares an oversize body and during reading otherwise.
- Reuse an invocation's immutable question map and preconvert valid static context once while keeping each row's exact request body, error ordering, cache identity, and dry-run output unchanged.
- Measure local row-preparation cost before and after the reuse change, and test size boundaries, chunked bodies, and request equivalence.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `jev-http-transport`: Bound successful response bodies for both authenticated endpoints before JSON decoding.

## Impact

The HTTP client, table request builder, typed request ownership, local tests, README, and repository usage skill are affected. The static-data reuse is internal; no table or request-shape requirement changes. The existing main specs remain unchanged until this change is synchronized or archived.
