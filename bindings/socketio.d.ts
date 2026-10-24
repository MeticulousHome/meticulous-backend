// The payloads of the Socket.IO events meticulous-backend emits, generated from
// the types next to the Rust code that builds each one (docs/rust/ARCHITECTURE.md).
// Do not edit: `UPDATE_CONTRACTS=1 cargo test -p meticulous-backend socketio_types`
// rewrites it; `meticulous-backend contract --typescript` prints it.

/** The `OSUpdate` event: the progress of an OS update. */
export interface OSUpdatePayload {
  /** Rounded to an integer. */
  progress: number;
  status: OSUpdateStatus;
  /** Empty, or ` : ` and a detail (the Python never sets one). */
  info: string;
}

export type OSUpdateStatus = "IDLE" | "DOWNLOADING" | "INSTALLING" | "COMPLETE" | "FAILED";

/** The `button` event: a physical button of the machine. */
export interface ButtonPayload {
  type: ButtonEventType;
  /** 10000 once the firmware's counter overflows (`9999+++`). */
  time_since_last_event: number;
}

export type ButtonEventType = "ENCODER_CLOCKWISE" | "ENCODER_COUNTERCLOCKWISE" | "ENCODER_PUSH" | "ENCODER_DOUBLE" | "ENCODER_LONG" | "TARE" | "TARE_DOUBLE" | "TARE_LONG" | "TARE_SUPER_LONG" | "CONTEXT" | "ENCODER_PRESSED" | "ENCODER_RELEASED" | "TARE_PRESSED" | "TARE_RELEASED" | "CONTEXT_PRESSED" | "CONTEXT_RELEASED" | "UNKNOWN";

/**
 * The `heater_status` event: the preheat time remaining; null for a value that is not
 * finite (Python wrote the invalid JSON tokens `NaN` and `Infinity`).
 */
export type HeaterStatusPayload = number | null;

/**
 * The `notification` event: the JSON text of a `NotificationMessage` (the Python emits
 * `json.dumps(...)`), not an object.
 */
export type NotificationPayload = string;

/** A notification, as GET /notifications lists it too. */
export interface NotificationMessage {
  id: string;
  message: string;
  image: string | null;
  responses: string[];
  /** Local time, `datetime.isoformat()` without an offset. */
  timestamp: string;
}

/**
 * The `profile` event: a profile was created, updated, deleted or loaded, or the whole
 * list changed (`full_reload`).
 */
export interface ProfilePayload {
  change: ProfileChange;
  /** Absent on `full_reload`. */
  profile_id?: string;
  /** The change log entry: on creates, updates and Pour Over deletes. */
  change_id?: string;
  /** Only on Pour Over profile events. */
  brew_type?: "pour_over";
}

export type ProfileChange = "create" | "update" | "delete" | "full_reload" | "load";

/**
 * The `profileHover` event: the profile a client has in focus, passed on to the other
 * clients as strings (`from` is `backend` in the hover a client gets on connect). Empty
 * strings for keys the client left out, and before any client sent one.
 */
export interface ProfileHoverPayload {
  id: string;
  type: string;
  from: string;
}

/**
 * The `report_progress` event, contract v1: where one report stands. Times are seconds
 * since the epoch.
 */
export interface ReportProgressPayload {
  v: 1;
  localID: string;
  origin: "dial" | "mobile";
  ticket: number | null;
  stage: ReportStage;
  /**
   * The stages this report runs: a Dial report has no `queued`, a mobile one no
   * `ticketing`.
   */
  stages: ReportPendingStage[];
  /** Debug shots copied so far, during `collecting_debug_shots`. */
  detail: { current: number; total: number } | null;
  cancellable: boolean;
  final: boolean;
  failure: { code: string; message: string } | null;
  startedAt: number;
  updatedAt: number;
}

export type ReportStage = ReportPendingStage | ReportFinalStage;

/** The stages that are not final, in the order a report runs them. */
export type ReportPendingStage = "queued" | "preparing" | "fetching_machine_info" | "fetching_logs" | "fetching_machine_status" | "collecting_debug_shots" | "ticketing" | "compressing" | "uploading" | "finishing";

export type ReportFinalStage = "submitted" | "failed" | "cancelled";

/**
 * The `sensors` event, after each `Sensors` frame. `safeFloat` sends 0 for a value that is
 * not finite, except in `motor_temp` and `weight_pred`.
 */
export interface SensorsPayload {
  t_ext_1: number;
  t_ext_2: number;
  t_bar_up: number;
  t_bar_mu: number;
  t_bar_md: number;
  t_bar_down: number;
  t_tube: number;
  t_motor_temp: number;
  lam_temp: number;
  p: number;
  a_0: number;
  a_1: number;
  a_2: number;
  a_3: number;
  m_pos: number;
  m_spd: number;
  m_pwr: number;
  m_cur: number;
  bh_pwr: number;
  bh_cur: number;
  w_stat: boolean;
  motor_temp: SensorReading;
  weight_pred: SensorReading;
}

/**
 * A reading the backend does not clamp: `"NaN"` where the firmware sent something that is
 * not a number (a failed sensor), `null` for an infinite one (Python wrote the invalid JSON
 * token `Infinity`).
 */
export type SensorReading = number | "NaN" | null;

/** The `settings` event: the settings were saved; re-read GET /settings. */
export type SettingsPayload = Record<string, never>;

/** The `status` event: the machine's last frame and the loaded profile. */
export interface StatusPayload {
  /** The firmware's status (`idle`, `heating`, ...); null in a five-field frame. */
  name: string | null;
  sensors: StatusSensors;
  setpoints: StatusSetpoints;
  time: number;
  /** The profile the firmware runs; null in a five-field frame. */
  profile: string | null;
  profile_time: number;
  state: MachineState;
  extracting: boolean;
  /** The name of the last profile loaded; null before the first. */
  loaded_profile: string | null;
  /** The id of that profile. */
  id: string | null;
}

/** Pressure, flow, weight, temperature and gravimetric flow. */
export interface StatusSensors {
  p: SensorReading;
  f: SensorReading;
  w: SensorReading;
  t: SensorReading;
  g: SensorReading;
}

/** The setpoint of each controller the frame names, by its kind in lower case. */
export interface StatusSetpoints {
  /** The kind in charge; null when the frame names none. */
  active: string | null;
  flow?: number;
  pressure?: number;
  piston?: number;
  power?: number;
  temperature?: number;
  /** A kind the firmware names that data.py does not list. */
  [kind: string]: number | string | null | undefined;
}

export type MachineState = "idle" | "purge" | "home" | "brewing";

/**
 * The `upload_report` event (and each entry of GET /reports/dispatch): a report the mobile
 * app dispatched, for the Dial to collect and upload. Times are seconds since the epoch.
 */
export interface UploadReportPayload {
  localID: string;
  /** The serial number; null on a machine without one. */
  machineID: string | null;
  ticket: number;
  issueTime: number;
  /** When the dispatch reached the backend. */
  requestTime: number;
  description: string | null;
  name: string | null;
  email: string | null;
}

/** Every event the backend emits, with its payload. */
export interface SocketIoPayloads {
  OSUpdate: OSUpdatePayload;
  button: ButtonPayload;
  heater_status: HeaterStatusPayload;
  notification: NotificationPayload;
  profile: ProfilePayload;
  profileHover: ProfileHoverPayload;
  report_progress: ReportProgressPayload;
  sensors: SensorsPayload;
  settings: SettingsPayload;
  status: StatusPayload;
  upload_report: UploadReportPayload;
}

/** The listeners, for socket.io-client's `Socket<ServerToClientEvents, ...>`. */
export type ServerToClientEvents = {
  [E in keyof SocketIoPayloads]: (data: SocketIoPayloads[E]) => void;
};
