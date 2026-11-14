"""Write transcode_goldens.json: JSON texts and what CPython makes of them.

For every text, `json.loads` either raises (the case records the exception
type) or reads a value, which the case records as `json.dumps` writes it:
`default` (ensure_ascii), `unicode` (ensure_ascii=False; null when the value
holds a lone surrogate that UTF-8 cannot encode; left out when equal to
`default`), `tornado` (tornado's json_encode, `</` escaped; left out when
equal to `default`), and `data`: `json.dumps(doc.get("data"))` for an
object, or `!<type name>` for any other document.

The texts are random documents written with random whitespace, escapes and
number spellings, with the tokens CPython reads that strict JSON parsers do
not (NaN, Infinity, -Infinity, big integers, lone surrogates), repeated keys,
and some of them mutated into invalid JSON. Run with any CPython 3.12:

    python3 crates/pyfmt/tests/data/generate_transcode_goldens.py
"""

import json
import math
import random
import struct
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent / "transcode_goldens.json"
rng = random.Random(20261024)

KEY_WORDS = [
    "data",
    "shot",
    "pressure",
    "flow",
    "time",
    "a",
    "b",
    "</x>",
    "é",
    "push_to_brew_time",
]
TEXT_CHARS = (
    list("abcxyz ABC 019")
    + list('"\\/<>')
    + ["</", "\n", "\t", "\r", "\b", "\f", "\x00", "\x1f", "\x7f"]
    + ["é", "ü", "ñ", "\u2028", "\u2615", "\ufeff", "\uffff", "😀", "𝄞"]
)


def random_float():
    kind = rng.random()
    if kind < 0.35:
        return round(rng.uniform(-300, 300), rng.choice([0, 1, 2, 3]))
    if kind < 0.5:
        return rng.uniform(-1, 1) * 10 ** rng.randint(-8, 20)
    if kind < 0.6:
        return rng.choice(
            [
                0.0,
                -0.0,
                0.1,
                0.30000000000000004,
                1e16,
                1e15,
                1e-5,
                1e-4,
                5e-324,
                1.7976931348623157e308,
            ]
        )
    if kind < 0.7:
        return rng.choice([math.nan, math.inf, -math.inf])
    bits = rng.getrandbits(64)
    value = struct.unpack("<d", struct.pack("<Q", bits))[0]
    return value


def random_int():
    kind = rng.random()
    if kind < 0.6:
        return rng.randint(-100000, 100000)
    if kind < 0.85:
        return rng.randint(-(2**63), 2**64)
    return rng.randint(-(10**60), 10**60)


def random_string():
    return "".join(rng.choice(TEXT_CHARS) for _ in range(rng.randint(0, 8)))


def random_value(depth=0):
    kind = rng.random()
    if depth < 4 and kind < 0.2:
        return {random_key(): random_value(depth + 1) for _ in range(rng.randint(0, 5))}
    if depth < 4 and kind < 0.35:
        return [random_value(depth + 1) for _ in range(rng.randint(0, 5))]
    if kind < 0.6:
        return random_float()
    if kind < 0.75:
        return random_int()
    if kind < 0.9:
        return random_string()
    return rng.choice([True, False, None])


def random_key():
    return rng.choice(KEY_WORDS) if rng.random() < 0.6 else random_string()


def ws():
    if rng.random() < 0.7:
        return ""
    return "".join(rng.choice(" \t\n\r") for _ in range(rng.randint(1, 3)))


SHORT = {
    '"': '\\"',
    "\\": "\\\\",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
    "\b": "\\b",
    "\f": "\\f",
}


def u_escape(unit):
    text = f"\\u{unit:04x}"
    return text.upper().replace("\\U", "\\u") if rng.random() < 0.3 else text


def write_string(s):
    parts = ['"']
    for c in s:
        code = ord(c)
        r = rng.random()
        if c in SHORT and (r < 0.6 or c in '"\\'):
            parts.append(SHORT[c] if r < 0.8 else u_escape(code))
        elif code < 0x20:
            parts.append(u_escape(code))
        elif c == "/" and r < 0.3:
            parts.append("\\/")
        elif code > 0xFFFF and r < 0.4:
            code -= 0x10000
            parts.append(u_escape(0xD800 + (code >> 10)) + u_escape(0xDC00 + (code & 0x3FF)))
        elif r < 0.15:
            parts.append(u_escape(code))
        else:
            parts.append(c)
    # Escapes CPython reads into lone surrogates.
    if rng.random() < 0.05:
        parts.append(
            rng.choice(["\\ud800", "\\udfff", "\\ud83d\\u0041", "\\udc00\\ud800", "\\uDBFF"])
        )
    parts.append('"')
    return "".join(parts)


def write_float(x):
    if math.isnan(x):
        return "NaN"
    if math.isinf(x):
        return "Infinity" if x > 0 else "-Infinity"
    r = rng.random()
    if r < 0.6:
        return repr(x)
    if r < 0.7:
        return f"{x:.17g}" if "e" in f"{x:.17g}" or "." in f"{x:.17g}" else f"{x:.17g}.0"
    if r < 0.8:
        text = f"{x:e}".replace("e", rng.choice(["e", "E"]))
        return text.replace("+", "") if rng.random() < 0.5 else text
    if r < 0.9:
        text = repr(x)
        return text + "0" * rng.randint(1, 3) if "e" not in text else text
    return rng.choice(
        ["1e400", "-1e400", "1e-400", "0e99999999999", "1E+2", "0.5e-3", "-0.0", "1.0E0"]
    )


def write_value(v):
    if isinstance(v, bool) or v is None:
        return json.dumps(v)
    if isinstance(v, int):
        return "-0" if v == 0 and rng.random() < 0.3 else str(v)
    if isinstance(v, float):
        return write_float(v)
    if isinstance(v, str):
        return write_string(v)
    if isinstance(v, list):
        if not v:
            return "[" + ws() + "]"
        return "[" + ws() + ("," + ws()).join(write_value(x) + ws() for x in v) + "]"
    items = [(k, x) for k, x in v.items()]
    if items and rng.random() < 0.2:
        items.append((items[0][0], random_value(3)))
    if not items:
        return "{" + ws() + "}"
    return (
        "{"
        + ws()
        + ("," + ws()).join(
            write_string(k) + ws() + ":" + ws() + write_value(x) + ws() for k, x in items
        )
        + "}"
    )


MUTATION_CHARS = list('"{}[],:\\ 0123456789-+.eEtfnNIau/')


def mutate(text):
    for _ in range(rng.randint(1, 2)):
        if not text:
            break
        i = rng.randrange(len(text))
        r = rng.random()
        if r < 0.4:
            text = text[:i] + text[i + 1 :]
        elif r < 0.8:
            text = text[:i] + rng.choice(MUTATION_CHARS) + text[i:]
        elif r < 0.9:
            text = text[:i] + rng.choice(MUTATION_CHARS) + text[i + 1 :]
        else:
            text = text[:i]
    return text


FIXED = [
    "",
    " ",
    "-0",
    "-0.0",
    "1E5",
    "1e+5",
    "1e400",
    "-1e400",
    "1e-400",
    "1e99999999999999999999",
    "0e99999999999",
    "12345678901234567890",
    "[NaN, Infinity, -Infinity]",
    '{"a": NaN}',
    '{"a": -Infinity}',
    '"\\ud800"',
    '"\\ud83d\\ude00"',
    '"\\ud83d\\u0041"',
    '"\\udc00\\ud800"',
    '"\\ud83d\\ude0"',
    '"\\ud83d\\ude0"x',
    '"\\ud83d\\u"',
    '"\\u12"',
    '{"a": 1, "b": 2, "a": 3}',
    '{"data": 1, "d\\u0061ta": 2}',
    '{"data": [1, 2], "x": [1,]}',
    "\ufeff{}",
    '"\\u00e9\u00e9</x>"',
    "1.",
    ".5",
    "01",
    "-",
    "- 1",
    "1e",
    "[1e]",
    "1e+",
    '"\x7f"',
    '"a\x1fb"',
    "[1,]",
    '{"a":1,}',
    " \t\n\r1 \t\n\r",
    '"\\/"',
    '"<\\/"',
    '"\\u003c/"',
    "nul",
    "NaNx",
    "[NaN1]",
    "-NaN",
    "-Inf",
    "infinity",
    "[-]",
    "[--1]",
    "[+1]",
    "[1.5.5]",
    "[0x10]",
    '{"a" 1}',
    "{1: 2}",
    "[true false]",
    '"abc',
    '"\\x"',
    '"\\',
    "[",
    "{",
    '{"a"',
    '{"a":',
    "[1 2]",
    "1 2",
    "[]]",
    "{}}",
    "\x00",
    "1" * 4300,
    "1" * 4301,
    "-" + "1" * 4300,
    "-" + "1" * 4301,
    "0." + "1" * 400,
    "[" * 127 + "]" * 127,
    '{"a": ' * 127 + "1" + "}" * 127,
    '"\\ud800\\udc00\\udbff\\udfff"',
    '"😀𝄞"',
    '{"\\ud800": 1, "data": "\\udfff"}',
    '{"x": {"a": [1], "\\u0061": {"b": 1, "b": 2}}, "y": 0, "x": 5}',
]


def record(text):
    case = {"text": text}
    try:
        value = json.loads(text)
    except (ValueError, RecursionError) as e:
        case["error"] = type(e).__name__
        return case
    default = json.dumps(value)
    case["default"] = default
    unicode = json.dumps(value, ensure_ascii=False)
    try:
        unicode.encode("utf-8")
    except UnicodeEncodeError:
        unicode = None
    if unicode != default:
        case["unicode"] = unicode
    tornado = default.replace("</", "<\\/")
    if tornado != default:
        case["tornado"] = tornado
    if isinstance(value, dict):
        case["data"] = json.dumps(value.get("data"))
    else:
        case["data"] = "!" + type(value).__name__
    return case


def main():
    texts = list(FIXED)
    for _ in range(2500):
        value = random_value()
        if rng.random() < 0.3:
            value = {"data": value, **({"push_to_brew_time": 0} if rng.random() < 0.5 else {})}
        text = ws() + write_value(value) + ws()
        texts.append(mutate(text) if rng.random() < 0.3 else text)
    cases = [record(text) for text in texts]
    OUT.write_text(
        json.dumps({"python": sys.version.split()[0], "cases": cases}, indent=0) + "\n"
    )
    errors = sum("error" in c for c in cases)
    print(f"{len(cases)} cases, {errors} refused, {OUT.stat().st_size} bytes")


if __name__ == "__main__":
    main()
