# Strict greater-than profile migration

Simplified profile exit triggers for weight, time, pressure, flow, piston position, and
power now use `>` for the UI choice labelled "Greater than". Missing comparisons also
default to `>`. During the compatibility window, the backend continues to accept legacy
`>=` input but canonicalizes it to strict `>` before validation, persistence, or conversion.

When the backend reads or receives a simplified profile, it migrates `>=` or an omitted
comparison to `>` only for those six exit-trigger types before validation. This migration
also applies to the cached last-profile record and is idempotent. It does not change `<=`,
limits, complex-profile operators, trigger values, relative flags, variables, IDs, ordering,
or timestamps.

This is a small boundary-condition behavior change: an exit trigger no longer activates
when its measured value is exactly equal to the threshold. In particular, a weight trigger
configured for greater than 0 g remains false at exactly 0 g and activates only after the
measured weight becomes positive.

Deploy the updated backend/schema and strict-comparison firmware together in the nightly
machine image before releasing clients that serialize `>` exclusively. Older clients remain
compatible during that rollout because their `>=` or omitted comparisons are canonicalized
at backend ingress.
