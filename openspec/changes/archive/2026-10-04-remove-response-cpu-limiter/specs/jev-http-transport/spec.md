# Spec Delta

## REMOVED Requirements

### Requirement: Bounded abandoned response work

**Reason**: The process-wide response-CPU limit adds waiting across otherwise
independent evaluations but cannot bound response bodies already buffered in
memory. Typical Jev responses do not reach the offload threshold.

**Migration**: No command or response migration is needed. The existing
per-evaluation deadline, 16 MiB response-body limit, and per-invocation
`--jobs` limit remain; there is no process-wide response-CPU guarantee.
