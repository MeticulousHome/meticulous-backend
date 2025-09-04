#!/usr/bin/env python3
import argparse
import json
import pprint
import sys


def get_nested(config, keys):
    """Retrieve a nested value using a list of keys."""
    for key in keys:
        if key in config:
            config = config[key]
        else:
            allowed_keys = "[" + ", ".join(config.keys()) + "]"
            raise KeyError(f"Key '{key}' not in {allowed_keys}")
    return config


def str2bool(v):
    return v.lower() in ("yes", "true", "t", "1")


def set_nested(config, keys, value):
    """Set a nested value using a list of keys, creating dictionaries as needed."""

    target = config[keys[0]][keys[1]]
    if isinstance(target, bool):
        config[keys[0]][keys[1]] = str2bool(value)
    elif isinstance(target, str):
        config[keys[0]][keys[1]] = value
    elif isinstance(target, int):
        config[keys[0]][keys[1]] = int(value)
    elif isinstance(target, float):
        config[keys[0]][keys[1]] = float(value)
    elif hasattr(target, "__len__"):  # target is list-alike
        # handle " and ' shell string escaping if non ambiguous
        if "'" in value and '"' not in value:
            value = value.replace("'", '"')
        data = json.loads(value)
        config[keys[0]][keys[1]] = data
    else:
        raise KeyError("Unsupported target type: ", type(target))


def parsePath(argpath):
    keys = argpath.split(".")
    if len(keys) >= 1 and keys[0] == "":
        keys = keys[1:]
    if keys[0] == "":
        return None
    else:
        return keys


def main():
    parser = argparse.ArgumentParser(
        description="Inspect and modify a configuration dictionary."
    )

    # Global flag for JSON output.
    parser.add_argument("--json", action="store_true", help="Output in JSON format")

    subparsers = parser.add_subparsers(dest="command", required=True, help="Sub-command to run")

    # 'get' command
    parser_get = subparsers.add_parser("get", help="Retrieve a configuration value.")
    parser_get.add_argument("keypath", help="Dot-separated key path (e.g., machine.version)")

    # 'set' command
    parser_set = subparsers.add_parser("set", help="Set a configuration value.")
    parser_set.add_argument("keypath", help="Dot-separated key path (e.g., machine.version)")
    parser_set.add_argument("value", help="Value to set")

    args = parser.parse_args()

    try:
        if args.command == "get":  #
            keys = parsePath(args.keypath)
            from config import MeticulousConfig

            if keys is None:
                result = MeticulousConfig
            else:
                result = get_nested(MeticulousConfig, keys)
            if args.json:
                print(json.dumps(result, indent=2))
            else:
                if isinstance(result, str):
                    print(result)
                else:
                    pprint.pprint(result)

        elif args.command == "set":
            keys = parsePath(args.keypath)
            if keys is None:
                raise Exception("Cannot set the root of the configuration.")

            from config import MeticulousConfig

            set_nested(MeticulousConfig, keys, args.value)
            result = get_nested(MeticulousConfig, keys)
            if args.json:
                print(json.dumps(result, indent=2))
            else:
                print(".".join(keys), "= ", end="")
                if isinstance(result, str):
                    print(result)
                else:
                    pprint.pprint(result)
            MeticulousConfig.save()
        else:
            parser.print_help()
            sys.exit(1)

    except Exception as e:
        print("Error:", e)
        import traceback

        traceback.print_exception(e)
        sys.exit(1)


if __name__ == "__main__":
    main()
