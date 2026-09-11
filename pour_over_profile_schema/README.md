# Meticulous Pour Over Profile Format — version 1

`schema.json` is the canonical machine contract for portable, guided Pour Over
recipes. The backend validates every profile against this schema and the
cross-field rules below before it writes anything to persistent storage. The
Dial only lists profiles that passed that boundary.

The running machine exposes the exact schema at:

`GET /api/v1/pour-over/profile/schema`

Community sends a profile with the same local-machine transport used by
Espresso:

`POST /api/v1/profile/save`

The JSON body is dispatched to the Pour Over validator when
`brew_type` is `pour_over`. A successful response means the profile was
validated and durably stored. Validation failures return HTTP 422 with a
`details` array containing a path, message, and code for each problem.

## Machine limits

- `version`: exactly `1`
- `brew_type`: exactly `pour_over`
- `id` and author IDs: UUIDs
- coffee dose: 5–40 g
- water temperature: 70–100 °C
- total water: greater than 0 and no more than 2,000 g
- pours: 1–5
- scheduled/profile time: no more than 600 seconds (the Dial also forces a
  live brew to end at 10:00)
- stored profiles: at most 100, each no more than 512 KiB after normalization
- `shortDescription`: at most 100 characters
- `description`: at most 500 characters
- portable machine image: optional compact JPEG data URI; decoded image no
  more than 300 KiB. Community should use the existing Espresso image uploader
  and machine-thumbnail preparation, which normalizes the source to JPEG.

The flow meter displays 0–10 g/s. Valid recipes may contain a higher value; the
Dial renders it as `10+` instead of expanding the live scale.

## Time and guidance semantics

`starts_at_s` is an absolute time on the single brew clock, measured from the
detected start of the first pour. Therefore the first pour is always `0`; later
values are the familiar `30`, `90`, etc. They are not countdowns.

`duration_s` is the intended active pouring duration. For example, a pour may
start at 30 seconds, add 50 g over 10 seconds, and then wait until the next
pour's absolute start time.

`flow_rate_g_s` is the target rate of water addition, not the pour pattern. The
optional `pattern` field describes center, spiral/circular, ring, edge, pulse,
or custom guidance. The current Dial guides start time, cumulative stop weight,
and flow. Pattern, direction, height, and notes remain portable recipe metadata
for Community and future Dial guidance.

## Required consistency rules

In addition to JSON Schema validation:

1. The first pour starts at 0 seconds.
2. Pour keys are unique.
3. Pours do not overlap: each `starts_at_s` is at or after the preceding start
   plus its `duration_s`.
4. `flow_rate_g_s` equals `water_g / duration_s`, within 2% or 0.1 g/s.
5. Every `target_cumulative_water_g` equals the sum of `water_g` through that
   pour, within 0.1 g.
6. `recipe.total_water_g` equals the sum of all pours, within 0.1 g.
7. A flow target sits inside its optional flow range.
8. Target brew time is not earlier than the end of the final pour, and the
   optional upper target is not earlier than the target.

See `example_profile.json` for a complete valid profile.
