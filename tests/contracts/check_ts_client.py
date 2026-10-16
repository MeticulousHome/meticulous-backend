"""Check meticulous-typescript-api against the backend's contract snapshots.

The Dial and the mobile app reach the backend through that client. This reads
its source (src/index.ts, src/types.ts) and fails when it relies on something
the backend does not provide:

* an HTTP call whose path and method no backend route serves,
* a Socket.IO event it subscribes to that the backend never emits,
* a key of a payload type (StatusData, SensorData, Temperatures) the backend's
  payload does not carry.

KNOWN_GAPS lists the mismatches present today. A listed gap that has been
closed also fails, so the list only shrinks deliberately.

Usage: python3 tests/contracts/check_ts_client.py --client PATH_TO_CHECKOUT
"""

import argparse
import json
import re
import sys
from pathlib import Path

CONTRACTS = Path(__file__).resolve().parent

KNOWN_GAPS = {
    "GET /api/v1/test/{}": "requestTest() calls a route the backend has never served",
    "event actuators": "the backend sends the motor and heater values inside 'sensors'",
    "event communication": "the backend sends the pressure and ADC values inside 'sensors'",
    "Temperatures.t_valv": "the backend's sensors payload has no valve temperature",
}

CALL = re.compile(
    r"axiosInstance\s*\.\s*(get|post|put|patch|delete)\s*(?:<[^>]*>)?\(\s*(`[^`]*`|'[^']*'|\"[^\"]*\")",
    re.S,
)
SUBSCRIPTION = re.compile(r"socket\s*\.\s*on\(\s*['\"]([A-Za-z_]+)['\"]")
INTERFACE = re.compile(r"export interface (\w+)\s*\{(.*?)\n\}", re.S)
FIELD = re.compile(r"^\s*(\w+)\??\s*:", re.M)

# TS payload types and the backend payload (socketio_payloads.json) holding their keys.
PAYLOAD_TYPES = {
    "StatusData": "status",
    "SensorData": "status.sensors",
    "Temperatures": "sensors",
}


def client_calls(index_ts: str) -> set[str]:
    calls = set()
    for method, literal in CALL.findall(index_ts):
        path = literal[1:-1]
        path = path.replace("${this.version}", "v1")
        path = re.sub(r"\$\{[^}]*\}", "{}", path)
        path = path.split("?", 1)[0]
        calls.add(f"{method.upper()} {path}")
    return calls


def backend_serves(call: str, routes: list[dict]) -> bool:
    method, path = call.split(" ", 1)
    # "0" fits every capture group in use: hex ids, (.*) and ([^/]+).
    concrete = path.replace("{}", "0")
    for route in routes:
        if method.lower() in route["methods"] and re.fullmatch(route["path"], concrete):
            return True
    return False


def interface_keys(types_ts: str) -> dict[str, set[str]]:
    return {name: set(FIELD.findall(body)) for name, body in INTERFACE.findall(types_ts)}


def find_gaps(client: Path) -> set[str]:
    index_ts = (client / "src" / "index.ts").read_text()
    types_ts = (client / "src" / "types.ts").read_text()
    routes = json.loads((CONTRACTS / "http_routes.json").read_text())
    events = json.loads((CONTRACTS / "socketio_events.json").read_text())
    payloads = json.loads((CONTRACTS / "socketio_payloads.json").read_text())

    gaps = set()
    calls = client_calls(index_ts)
    if len(calls) < 40:
        # The client has ~50 calls; far fewer means the pattern stopped matching.
        gaps.add(f"only {len(calls)} HTTP calls recognised in index.ts")
    for call in calls:
        if not backend_serves(call, routes):
            gaps.add(call)
    for event in set(SUBSCRIPTION.findall(index_ts)):
        if event not in events["emits"]:
            gaps.add(f"event {event}")
    interfaces = interface_keys(types_ts)
    for type_name, payload in PAYLOAD_TYPES.items():
        if type_name not in interfaces:
            gaps.add(f"{type_name} (type not found in types.ts)")
            continue
        for key in interfaces[type_name] - set(payloads[payload]):
            gaps.add(f"{type_name}.{key}")
    return gaps


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--client", type=Path, required=True)
    args = parser.parse_args()

    gaps = find_gaps(args.client)
    problems = []
    for gap in sorted(gaps - KNOWN_GAPS.keys()):
        problems.append(f"the client relies on {gap}, which the backend does not provide")
    for gap in sorted(KNOWN_GAPS.keys() - gaps):
        problems.append(f"known gap '{gap}' is closed: remove it from KNOWN_GAPS")

    for gap in sorted(gaps & KNOWN_GAPS.keys()):
        print(f"known gap: {gap} ({KNOWN_GAPS[gap]})")
    for problem in problems:
        print(f"::error::{problem}")
    if problems:
        sys.exit(1)
    print("the TypeScript client only relies on what the backend provides")


if __name__ == "__main__":
    main()
