"""Read the backend's external contracts out of its source.

The Dial, the mobile app and meticulous-typescript-api talk to the backend over
HTTP and Socket.IO and read its settings by name. tests/test_api_contracts.py
compares what these functions find with the snapshots next to this file, so a
change to any of them shows up in the pull request as a snapshot diff.

Routes and events are read from the AST rather than by importing the API
modules, which pull in the machine-only dependency group.
"""

import ast
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
HTTP_METHODS = ("get", "post", "put", "patch", "delete")

# Handlers inherited from tornado and the methods they serve.
TORNADO_BASES = {
    "StaticFileHandler": ["get"],
    "RedirectHandler": ["get"],
}

# Source the backend ships (see Dockerfile.deb); tests and submodules are not API.
SOURCE_EXCLUDES = ("tests", "log_redactor", "alembic", ".venv", "profile_schema")


def _source_files():
    for path in sorted(REPO_ROOT.rglob("*.py")):
        relative = path.relative_to(REPO_ROOT)
        if relative.parts[0] in SOURCE_EXCLUDES:
            continue
        yield relative, ast.parse(path.read_text(), filename=str(relative))


def _base_name(node: ast.expr) -> str:
    if isinstance(node, ast.Attribute):
        return node.attr
    if isinstance(node, ast.Name):
        return node.id
    return ""


def _handler_methods(classes: dict, name: str, seen=None) -> set:
    seen = seen or set()
    if name in seen:
        return set()
    seen.add(name)
    if name in TORNADO_BASES:
        return set(TORNADO_BASES[name])
    node = classes.get(name)
    if node is None:
        return set()
    methods = {
        item.name
        for item in node.body
        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
        and item.name in HTTP_METHODS
    }
    for base in node.bases:
        methods |= _handler_methods(classes, _base_name(base), seen)
    return methods


def http_routes() -> list[dict]:
    """Every API.register_handler(APIVersion.V1, pattern, Handler) call under api/."""
    modules = [
        (relative, tree) for relative, tree in _source_files() if relative.parts[0] == "api"
    ]
    # Handlers are imported between api modules (emulation.py reuses wifi.py's),
    # so a name not defined locally is looked up across the package.
    package_classes = {}
    for _, tree in modules:
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                package_classes.setdefault(node.name, node)

    routes = []
    for relative, tree in modules:
        classes = dict(package_classes)
        classes.update(
            {node.name: node for node in ast.walk(tree) if isinstance(node, ast.ClassDef)}
        )
        for node in ast.walk(tree):
            if not (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and node.func.attr == "register_handler"
                and len(node.args) >= 3
                and isinstance(node.args[1], ast.Constant)
            ):
                continue
            version = _base_name(node.args[0]).lower()
            handler = _base_name(node.args[2])
            routes.append(
                {
                    "path": f"/api/{version}{node.args[1].value}",
                    "handler": handler,
                    "module": relative.with_suffix("").as_posix().replace("/", "."),
                    "methods": sorted(_handler_methods(classes, handler)),
                }
            )
    return sorted(routes, key=lambda route: (route["path"], route["module"]))


def socketio_events() -> dict:
    """Events the server listens for (backend.py) and every event name it emits."""
    listens, emits = set(), set()
    for relative, tree in _source_files():
        # Event names held in module constants, e.g. UPLOAD_REPORT_EVENT.
        constants = {
            target.id: node.value.value
            for node in tree.body
            if isinstance(node, ast.Assign)
            and isinstance(node.value, ast.Constant)
            and isinstance(node.value.value, str)
            for target in node.targets
            if isinstance(target, ast.Name)
        }
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and relative.name == (
                "backend.py"
            ):
                for decorator in node.decorator_list:
                    if (
                        isinstance(decorator, ast.Call)
                        and _base_name(decorator.func) == "on"
                        and decorator.args
                        and isinstance(decorator.args[0], ast.Constant)
                    ):
                        listens.add(decorator.args[0].value)
                    elif _base_name(decorator) == "event":
                        listens.add(node.name)
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and node.func.attr == "emit"
                and node.args
            ):
                event = node.args[0]
                if isinstance(event, ast.Constant) and isinstance(event.value, str):
                    emits.add(event.value)
                elif isinstance(event, ast.Name) and event.id in constants:
                    emits.add(constants[event.id])
                else:
                    raise ValueError(
                        f"{relative}:{node.lineno}: emit() with an event name this "
                        "extractor cannot resolve; use a literal or a module constant"
                    )
    return {"listens": sorted(listens), "emits": sorted(emits)}


def payload_keys() -> dict:
    """Keys of the Socket.IO payloads clients read by name."""
    from esp_serial.data import (
        ButtonEventData,
        ButtonEventEnum,
        SensorData,
        ShotData,
    )

    status = ShotData().to_sio()
    return {
        # backend.live() adds the loaded profile's name and id to every status.
        "status": sorted(set(status) | {"loaded_profile", "id"}),
        "status.sensors": sorted(status["sensors"]),
        "sensors": sorted(SensorData().to_sio_sensors()),
        "button": sorted(ButtonEventData(event=next(iter(ButtonEventEnum))).to_sio()),
    }


def settings_defaults() -> dict:
    """The default configuration: every key, and the user section's default values."""
    from config import CONFIG_USER, DefaultConfiguration_V1

    contract = {}
    for section, values in DefaultConfiguration_V1.items():
        if not isinstance(values, dict):
            contract[section] = values
        elif section == CONFIG_USER:
            contract[section] = dict(sorted(values.items()))
        else:
            contract[section] = sorted(values)
    return contract


SNAPSHOTS = {
    "http_routes.json": http_routes,
    "socketio_events.json": socketio_events,
    "socketio_payloads.json": payload_keys,
    "settings_defaults.json": settings_defaults,
}
