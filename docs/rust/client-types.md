# Socket.IO payload types: the backend and the TypeScript client

`bindings/socketio.d.ts` is generated from the types next to the code that
builds each Socket.IO payload (see "Socket.IO payload types" in
`ARCHITECTURE.md`). This page compares it with the types the Dial and the
mobile app use: `src/types.ts` of
[meticulous-typescript-api](https://github.com/MeticulousHome/meticulous-typescript-api)
at `99edf03` (main, "feat: report progress types, event and methods").
References are `types.ts:<line>` and `index.ts:<line>` in that commit.

The comparison was made with the TypeScript compiler (5.9, `--strict`):
each backend payload type assigned to the client's type for the same event,
and back, key by key. Both sides list the same eleven events
(`ServerToClientEvents`, `types.ts:590`).

## Where the client type is narrower than the wire

The backend sends values the client's type does not admit. Code that trusts
the type can break on them.

| Event | Key | Client | Backend | When |
|---|---|---|---|---|
| `status` | `name` | `string` (`types.ts:446`) | `string \| null` | a Data frame of five fields |
| `status` | `profile` | `string` (`types.ts:450`) | `string \| null` | a Data frame of five fields |
| `status` | `loaded_profile` | `string` (`types.ts:451`) | `string \| null` | no profile loaded since the settings were created |
| `status` | `id` | `string` (`types.ts:452`) | `string \| null` | same |
| `status` | `sensors.p`, `.f`, `.w`, `.t`, `.g` | `number` (`SensorData`, `types.ts:426-432`) | `number \| "NaN" \| null` | `"NaN"`: the firmware sent `nan` or something that is not a number (a failed sensor); `null`: an infinite value |
| `status` | `setpoints.active` | `active?: string` (`types.ts:437`) | `string \| null`, always present | `null` when the frame names no controller in charge |
| `sensors` | `motor_temp`, `weight_pred` | `number` (`types.ts:544-545`) | `number \| "NaN" \| null` | as `status.sensors`; the other 20 values are clamped to numbers (`safeFloat` sends 0) |
| `heater_status` | the payload | `number` (`HeaterStatus`, `types.ts:574`) | `number \| null` | an infinite or NaN `preheat_remaining` |
| `notification` | `image` (in the JSON text) | `image?: string` (`types.ts:36`) | `string \| null`, always present | `null` for a notification without an image |
| `profileHover` | `from` | `'dial' \| 'app' \| 'backend'` (`types.ts:502, 513`) | `string` | whatever a client sent is passed on |
| `profileHover` | `type` | `'focus' \| 'scroll'` (`types.ts:504, 514`) | `string` | as `from`; `""` (and `id: ""`) in the hover a client gets on connect before any client sent one |

The `null`s for infinite values are the Rust backend's: Python wrote the
invalid JSON tokens `Infinity` and `NaN` there (`heater_status` and the
readings above), which `JSON.parse`, and so socket.io-client's default
parser, rejects.

## Where the client declares more than the wire carries

Harmless for a client, but the type promises values or keys that never
arrive.

| Event | Key | Client | Backend |
|---|---|---|---|
| `status` | `state` | includes `'error'` (`MachineState`, `types.ts:434`) | `"idle" \| "purge" \| "home" \| "brewing"`; Python marks `MachineState.ERROR` "so far unused" |
| `notification` | `responses` | optional (`types.ts:37`) | always present (`["Ok"]` by default) |
| `profile` | `brew_type` | `'espresso' \| 'pour_over'` (`BrewType`, `types.ts:492, 499`) | only `"pour_over"`; espresso profile events have no `brew_type` |
| `OSUpdate` | `progress`, `status`, `info` | all optional, `status?: string` (`types.ts:727-731`) | all always present; `status` is `"IDLE" \| "DOWNLOADING" \| "INSTALLING" \| "COMPLETE" \| "FAILED"` |
| `report_progress` | `stages` | `ReportStage[]` (`types.ts:384`) | only stages that are not final (`ReportPendingStage[]`) |
| `sensors` (as `Temperatures`) | `t_valv` | required (`types.ts:466`) | never sent; already a known gap in `tests/contracts/check_ts_client.py` |

Matching exactly: `button` (and all 17 `ButtonEventType` values),
`settings`, `upload_report`, every other key of `report_progress` (`v: 1`
included), `profile.change`, and the setpoint kinds (`flow`, `pressure`,
`piston`, `power`, `temperature`, `types.ts:438-442`). The backend also
passes on any other controller kind the firmware names, as another key of
`setpoints` (an index signature in the bindings); the client has no such
key, which TypeScript tolerates.

## The client library itself

- `index.ts:174` keeps an untyped `Socket`, so `ServerToClientEvents` is
  not enforced by the library: `socket.on` takes any listener.
- `index.ts:64` and `index.ts:215` subscribe `onTemperatures(data:
  Temperatures)` to `sensors`, whose payload is `MachineSensors` (no
  `t_valv`, 16 more keys).
- `index.ts:68` declares `onNotification(data: NotificationItem)`, but the
  `notification` event carries the JSON *text* of a `NotificationItem`
  (`types.ts:596-597` says so). Neither `onNotification` nor
  `onProfileUpdate` (`index.ts:67`) is ever subscribed.
- `index.ts:218-224` subscribe to `communication` and `actuators`, which no
  backend emits (known gaps in `check_ts_client.py`).

## Found on the backend side

- `profileHover` is not at parity: `ProfileHover.from_dict` in Python passes
  a client's values through as they are (`{"id": 5}` reaches the other
  clients as `5`, `"type": null` as `null`), while `Hover::from_value`
  (`crates/backend/src/profiles.rs`) turns them into strings (`"5"`, `""`).
  The bindings describe the Rust behaviour (`string`); the payload was left
  as it is.

## Not declared, on purpose

- `report_progress.ticket` can be `true` or `false`: a draft PUT accepts a
  `ticket` of any type, and the progress takes it when it passes Python's
  `isinstance(ticket, int)`, which a bool does. Only a client that
  sends a boolean ticket gets one back; the bindings keep `number | null`,
  and a test that exercised it would fail the check every emit gets in test
  builds.
- `status.name` and `status.profile` are `null` only for a Data frame of
  five fields, which the firmware does not normally send; they stay
  nullable because the parser accepts such frames.

## Reproducing

Copy `bindings/socketio.d.ts` and the client's `src/types.ts` (with the
`@meticulous-home/espresso-profile` import replaced by any `Profile` type)
next to a file that assigns each backend payload to the client type and
back, and run `tsc --strict --noEmit` on it:

```ts
import type * as B from './socketio';
import type * as C from './types';
declare const b: B.SocketIoPayloads;
export const status: C.StatusData = b.status; // an error: the client type is narrower
declare const c: C.StatusData;
export const back: B.StatusPayload = c; // an error: the client declares more
```
