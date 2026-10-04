# Design

## Context

See [proposal.md](proposal.md) for motivation. A Nu `Record` can contain repeated field names even though a JSON object or Rust map cannot. The plugin accepts user-authored question records and converts selected Nu values into JSON before HTTP dispatch. Nu also reads each command's declared input/output types from its registered signature. The corresponding boundary fixes already exist as uncommitted worktree changes; this design records the intended approach for review and verification.

## Goals / Non-Goals

**Goals:**

- Detect information loss at the Nu-to-JSON boundary before map insertion or HTTP dispatch.
- Reject unsupported question fields while preserving flexible structured descriptions.
- Advertise only stable Nu type shapes; keep runtime checks for value-level constraints.

**Non-Goals:**

- Changing API request or response schemas, model selection, retry behavior, or table scheduling.
- Forcing every source row field through conversion when `--state` or `--fields` selects a subset.
- Statically encoding dynamic answer names, flag-dependent output fields, or row error-policy shapes in Nu signatures.

## Decisions

### Validate before lossy map construction

Question-name uniqueness is checked while iterating the original Nu record, before building the name-keyed question map. The shared recursive Nu-to-JSON converter rejects repeated keys in every selected record and reports the key path. The same converter is used for raw question bodies, state, and explicit context, so nested duplicates are not silently collapsed. An alternative would check only the final JSON map, but by then the overwritten value is already lost.

### Reject unknown question fields at the typed boundary

Deserializing a raw question into its Noul, Choice, or Score type rejects unrecognized top-level fields. Existing Noul criteria validation retains its `true`/`false` restriction, while structured `instructions` and descriptions remain arbitrary supported JSON trees. This catches typos without creating a separate, divergent list of field names in command code. The alternative, ignoring unknown fields for forward compatibility, risks sending a different question from the one the caller intended.

### Make registered signatures conservative and enforce no-input commands

Declare `nothing -> string` for root guidance, `any -> record` for `ask`, `any -> list<any>` for `annotate`, and `nothing -> record` for `models` and the three constructors. `any` is intentional: `ask` accepts several state forms and `annotate --on-error keep` may emit an unchanged non-record value. Dynamic answer and destination fields are not represented as fixed record columns. Root guidance and constructors also check runtime input so a caller cannot silently lose a piped value. The alternative of claiming all annotation rows are records would contradict the existing keep policy.

## Risks / Trade-offs

- Stricter validation may reject previously tolerated typos or forward API extensions → document the intentional break and update typed question support when the service schema changes.
- The conservative `any` signatures cannot prove all runtime constraints statically → retain existing state, row, and question validation with actionable errors.
- Error paths may contain user-defined field names → include paths but never field values or request bodies in validation errors.

## Migration Plan

Review the existing worktree implementation against these deltas, add or adjust focused boundary tests, and run the repository's Rust checks. Users must remove unsupported question fields and duplicate outbound keys; they must pass constructor instructions as arguments and invoke bare `jev` without pipeline input. No stored data or service-side migration is needed. Reverting this change would restore permissive parsing and signatures, but would reintroduce silent input loss.
