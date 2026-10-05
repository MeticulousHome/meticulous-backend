# Profile exit-trigger comparisons

Simplified profile exit triggers for weight, time, pressure, flow, piston position, and
power support four comparison operators. The backend preserves each explicit operator when
it stores a profile and maps it to the same complex-profile operator during conversion:

- `>` is false at the threshold and true above it.
- `<` is false at the threshold and true below it.
- `>=` is true at and above the threshold.
- `<=` is true at and below the threshold.

The `comparison` field remains optional for compatibility. An omitted comparison maps to
`>=`, matching the existing converter behavior. No supported explicit comparison is legacy,
and the backend does not migrate stored profiles from one operator to another.

For example, a weight trigger configured as `> 0 g` remains false at exactly 0 g and becomes
true after the measured weight is positive. A `>= 0 g` trigger remains true at exactly 0 g.
