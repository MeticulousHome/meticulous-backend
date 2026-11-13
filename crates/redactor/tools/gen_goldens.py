"""Differential corpus for crates/redactor.

Runs the Python implementation -- log_redactor.redact() and the backend's
LogRedactionFilter -- over random, realistic log text and stores what it
produced. tests/differential.rs asserts the Rust port produces exactly the
same output for every case.

    uv run python crates/redactor/tools/gen_goldens.py
        regenerates crates/redactor/tests/data/*.jsonl (the committed corpus)

    uv run python crates/redactor/tools/gen_goldens.py --scale 25 --seed 7 --out /tmp/big
    REDACTOR_GOLDENS_DIR=/tmp/big cargo test -p redactor --release --test differential
        a much larger corpus, not committed

Python sorts learned SSIDs out of a set, so when two of equal length overlap
the result depends on PYTHONHASHSEED. Every case is computed under several
hash seeds and only the ones Python itself agrees on become goldens.

Non-ASCII characters are written as chr() calls on purpose: this file must
stay plain ASCII to survive the tools that edit it.
"""

import argparse
import json
import os
import random
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
DATA = os.path.join(HERE, "..", "tests", "data")
HASH_SEEDS = ("0", "1", "2", "3")

U = chr  # shorthand for the non-ASCII characters below

# --------------------------------------------------------------------------
# value pools
# --------------------------------------------------------------------------

SSIDS = [
    "HomeNet",
    "Coffee Bar 2.4GHz",
    "TP-Link_5G",
    "MOVISTAR_1A2B",
    "Casa de Ana",
    "FRITZ!Box 7590",
    "Some Network 5G",
    "OfficeNet",
    "HomeNet5G",
    "Home",
    "net-work",
    "my_net",
    "TELLO-ABCDEF",
    "Caf" + U(0xE9) + " Wi-Fi",
    U(0x7F51) + U(0x7EDC) + "-Home",
    U(0x130) + "stanbul Net",
    U(0x17F) + "sid",
    "Kelvin " + U(0x212A),
    U(0xDC) + "n" + U(0xEF) + "c" + U(0xF8) + "d" + U(0xE9),
    "a'b",
    'x"y',
    "up",
    "ok",
    "lo",
    "eth0",
    "usb1",
    "Wired connection 1",
    "Bar",
    "123456",
    "x" * 32,
    "y" * 40,
    "[SSID_604cf397]",
    "ssid: nested",
    "Guest #2",
    "Net;Work",
    "Net,Work",
    "Net}Work",
    "Net]Work",
    "  padded  ",
    "tab\there",
    "Linksys",
    "linksys",
    "NETGEAR42",
    "AndroidAP" + U(0x663),
    "iPhone de Mar" + U(0xED) + "a",
    "Belkin.setup",
    "eir-ABCD",
    "Home-Net",
    "Home_Net",
    "Net",
    "Network",
    "Network1",
    "Network10",
]

CRED_VALUES = [
    "s3cr3tvalue",
    "hunter2",
    "sup3rs3cret",
    "p@ss|word",
    "a.b*c+d?",
    "pass word",
    "abc123",
    "[REDACTED]",
    "[redacted]",
    "[MAC_0123abcd]",
    "[SSID_ABCDEF12]",
    "12345678",
    "pw",
    "contrase" + U(0xF1) + "a",
    U(0x5BC6) + U(0x7801) + "-p" + U(0xE4) + "ss",
    "x,y",
    "x;y",
    "x}y",
    "x]y",
    "aa:bb:cc:dd:ee:ff",
    "fe80::1",
    "HomeNet",
    "Bearer abc.def.ghi",
    "eyJhbGciOiJIUzI1NiJ9.e30.sig",
]

CRED_KEYS = [
    "root_password",
    "rootpassword",
    "ROOT_PASSWORD",
    "RootPassword",
    "APPassword",
    "ap_password",
    "appassword",
    "passwd",
    "password",
    "Password",
    "PASSWORD",
    "passphrase",
    "psk",
    "PSK",
    "pre_shared_key",
    "preshared_key",
    "preshared",
    "secret",
    "Secret",
    "token",
    "access_token",
    "token_id",
    "api_key",
    "api-key",
    "apikey",
    "API_KEY",
    "authorization",
    "Authorization",
    "bearer",
    "auth_key",
    "my_password",
    "password_hash",
    "xpassword",
    "s" + U(0x130) + "cret",
    "secr" + U(0x131) + "t",
    "to" + U(0x212A) + "en",
    U(0x17F) + "ecret",
    "pa" + U(0x17F) + U(0x17F) + "word",
    "ap" + U(0x131) + "key",
]

SEPARATORS = [
    ": ",
    ":",
    "=",
    " = ",
    " :",
    ":  ",
    "\t:\t",
    ": " + U(0xA0),
    ":" + U(0x1F),
    " - ",
    " ",
]

AREAS = [
    "Africa",
    "America",
    "Antarctica",
    "Arctic",
    "Asia",
    "Atlantic",
    "Australia",
    "Brazil",
    "Canada",
    "Chile",
    "Etc",
    "Europe",
    "Indian",
    "Mexico",
    "Pacific",
    "US",
    "Mars",
    "america",
    "EUROPE",
]

LOCATIONS = [
    "Mexico_City",
    "Berlin",
    "Argentina/Buenos_Aires",
    "Kolkata",
    "New_York",
    "GMT-6",
    "GMT+12",
    "GMT-14",
    "GMT+1" + U(0x663),
    "UTC",
    "GMT",
    "Universal",
    "Zulu",
    "Greenwich",
    "Indiana/Indianapolis",
    "Port-au-Prince",
    "Foo1",
    "St_Johns",
    "Kiev",
    "_",
    "a/b/c",
    "Berlin" + U(0xE9),
]

TZ_SUFFIXES = [
    "",
    "",
    " (CST)",
    "' (CST).",
    '" (CEST, +0200)',
    " (ART, -0300)",
    " (OOM)",
    " ()",
    " (" + "X" * 40 + ")",
    "x",
    "-6x",
    "/",
    "/Mitte",
    "/Mitte9",
    "9",
    "_x",
    " (CST",
    "'(PST)",
]

WORDS = [
    "wifi",
    "INFO",
    "DEBUG",
    "WARNING",
    "ERROR",
    "machine",
    "connection",
    "state",
    "idle",
    "sensor",
    "profile",
    "default",
    "connected",
    "failed",
    "to",
    "with",
    "for",
    "on",
    "the",
    "network",
    "known",
    "Found",
    "WIFI",
    "Connecting",
    "renamed",
    "today",
    "address",
    "record",
    "via",
    "is",
    "up",
    "and",
    "running",
    "ok",
    "Home",
    "Net",
    "-",
    ":",
    "=",
    "/",
    "(",
    ")",
    "'",
    '"',
]

TRICKY = list("aAzZ09_-:=,;}]['\"#/*()+ \t.") + [
    "\r",
    "\n",
    "\r\n",
    "\x0b",
    "\x0c",
    "\x1c",
    "\x1d",
    "\x1e",
    "\x1f",
    U(0x85),
    U(0xA0),
    U(0x2028),
    U(0x2029),
    U(0x3000),
    U(0x200B),
    U(0x301),
    U(0xB2),
    U(0x663),
    U(0x130),
    U(0x131),
    U(0x17F),
    U(0x212A),
    U(0xE9),
    U(0xDF),
    U(0x7F51),
    U(0x1F600),
    U(0x16EE),
    U(0xAA),
    U(0xB7),
    U(0xAD),
    U(0x1680),
]

LINE_BREAKS = (
    ["\n"] * 12 + ["\r\n"] * 3 + ["\r", "\x0b", "\x0c", "\x1c", "\x1e", U(0x85), U(0x2028)]
)

PREFIXES = [
    "",
    "2026-07-28 01:42:05.388326+00:00 ",
    "2026-07-28 01:42:05,388 wifi INFO ",
    "Jul 28 01:42:05 meticulous332233 NetworkManager[812]: <info>  [1722130925.3883] ",
    "Jul 28 01:42:05 meticulous332233 wpa_supplicant[790]: wlan0: ",
    "config DEBUG CONF: ",
    "tornado.access INFO 304 GET /api/v1/settings/ (10.10.0.79) 4.10ms ",
]


# --------------------------------------------------------------------------
# generators
# --------------------------------------------------------------------------


def hexstr(r, n):
    s = "".join(r.choice("0123456789abcdef") for _ in range(n))
    return s.upper() if r.random() < 0.3 else s


def mac(r):
    roll = r.random()
    if roll < 0.15:
        return r.choice(
            [
                "ff:ff:ff:ff:ff:ff",
                "00:00:00:00:00:00",
                "33:33:00:00:00:01",
                "01:00:5e:00:00:fb",
                "FF:FF:FF:FF:FF:FF",
                "01:00:5E:7F:FF:FA",
            ]
        )
    sep = ":" if roll < 0.9 else r.choice(["-", "_", "."])
    groups = [hexstr(r, 2) for _ in range(6)]
    if r.random() < 0.1:
        groups.append(hexstr(r, 2))
    if r.random() < 0.05:
        groups[r.randrange(6)] = hexstr(r, 3)
    return sep.join(groups)


def ipv6(r):
    roll = r.random()
    groups = [hexstr(r, r.randint(1, 4)).lstrip("0") or "0" for _ in range(8)]
    if roll < 0.2:
        out = ":".join(groups)
    elif roll < 0.5:
        a = r.randint(1, 6)
        b = r.randint(a + 1, 8)
        out = ":".join(groups[:a]) + "::" + ":".join(groups[b:])
    elif roll < 0.6:
        out = "::" + ":".join(groups[r.randint(1, 8) :])
    elif roll < 0.65:
        out = ":".join(groups[: r.randint(1, 7)]) + "::"
    elif roll < 0.72:
        out = r.choice(["::1", "::", "::ffff:10.0.0.1", "fe80::1%wlan0", "[2001:db8::1]:8080"])
    elif roll < 0.8:
        out = "fe80::" + ":".join(hexstr(r, 4) for _ in range(4))
    elif roll < 0.9:
        # Malformed on purpose: 5-digit groups, 9 groups, triple colons.
        out = r.choice(
            [
                ":".join(groups) + ":" + hexstr(r, 2),
                hexstr(r, 5) + "::" + hexstr(r, 2),
                "fe80:::1",
                "::" + hexstr(r, 5),
                ":".join(groups[:7]) + "::" + groups[7],
                "1::2::3",
                "::abcd:1234x",
                "abcd::1234_",
            ]
        )
    else:
        out = "%02d:%02d:%02d" % (r.randrange(24), r.randrange(60), r.randrange(60))
    return out


def ipv4(r):
    return ".".join(str(r.randrange(256)) for _ in range(4))


def email(r):
    user = r.choice(["user", "john.doe", "a+b", "x_y"])
    return f"{user}@{r.choice(['example.com', 'mail.example.org', 'meticulous.coffee'])}"


def timezone(r):
    tz = f"{r.choice(AREAS)}/{r.choice(LOCATIONS)}"
    return tz + r.choice(TZ_SUFFIXES)


def cred_value(r):
    value = r.choice(CRED_VALUES)
    roll = r.random()
    if roll < 0.25:
        return f"'{value}'"
    if roll < 0.45:
        return f'"{value}"'
    if roll < 0.5:
        return r.choice(["'", '"']) + value
    if roll < 0.55:
        return ""
    return value


def cred_kv(r):
    key = r.choice(CRED_KEYS)
    roll = r.random()
    if roll < 0.2:
        return "{" + f'"{key}": "{r.choice(CRED_VALUES)}", "serial": "332233"' + "}"
    if roll < 0.3:
        return "{" + f"'{key}': '{r.choice(CRED_VALUES)}'" + "}"
    if roll < 0.37:
        return f"Config: added '{key}' value '{r.choice(CRED_VALUES)}'"
    if roll < 0.42:
        return f"ADDED  '{key}'  VALUE '{r.choice(CRED_VALUES)}'"
    if roll < 0.47:
        return r.choice(
            [
                "Password already set: True",
                "pam_unix(chpasswd:chauthtok): password changed for root",
                "Listening on gpg-agent.socket - passphrase cache.",
                "Config: added 'key_mgmt' value 'WPA-PSK WPA-PSK-SHA256 FT-PSK'",
            ]
        )
    if roll < 0.52:
        return f"Authorization: Bearer {r.choice(CRED_VALUES)}"
    quote = r.choice(["", "", "", "'", '"'])
    return f"{quote}{key}{quote}{r.choice(SEPARATORS)}{cred_value(r)}"


def ssid_line(r, ssid):
    templates = [
        "Config: added 'ssid' value '{s}'",
        "wlan0: Trying to associate with SSID '{s}'",
        'Connected to wireless network "{s}"',
        "SME: Trying to authenticate with {mac} (SSID='{s}' freq=2437 MHz)",
        "policy: set '{s}' (wlan0) as default for IPv4 routing and DNS",
        "device (wlan0): selected access point '{s}'",
        "device (wlan0): Activation: starting connection '{s}' (uuid)",
        "ssid: {s}",
        "SSID: {s}",
        "ssid='{s}'",
        'ssid = "{s}"',
        '{{"ssid": "{s}", "bssid": "{mac}"}}',
        "ssid={s}, freq=2412",
        "ssid:{s}#comment",
        "xssid: {s}",
        "my_ssid: {s}",
        U(0x17F) + "sid: {s}",
        "S" + U(0x130) + "D: {s}",
        "user renamed network to {s} today",
        "Found known WIFI {s}. Connecting",
        "noaccess point '{s}'",
        "reconnection '{s}'",
        'wireless network "{s}',
        "SSID='{s}' SSID='{s}'",
        "associate with SSID '{s}' and ssid: {s}",
        "{s}",
        "-{s}-",
        "_{s}_",
        "({s})",
    ]
    return r.choice(templates).format(s=ssid, mac=mac(r))


def conf_dump(r, ssids):
    """A backend config dump, one record per line, with a KnownWifis block."""
    indent = r.choice([" ", "  ", "   ", "\t", U(0x3000)])
    base = r.randint(0, 3)
    lines = [indent * base + "wifi:"]
    inline = r.choice(["", "", "", " |", " >", " {}", " x"])
    lines.append(indent * (base + 1) + "KnownWifis:" + inline)
    for ssid in r.sample(ssids, r.randint(0, min(3, len(ssids)))):
        lines.append(indent * (base + 2) + ssid + r.choice([":", ":", "  :", ": {}", ":x"]))
        if r.random() < 0.8:
            lines.append(indent * (base + 3) + f"password: {r.choice(CRED_VALUES)}")
        if r.random() < 0.5:
            lines.append(indent * (base + 3) + "last_used: yesterday")
        if r.random() < 0.1:
            lines.append(indent * (base + 4) + "nested: deeper")
    if r.random() < 0.3:
        lines.append(indent * (base + 2) + "# comment")
    if r.random() < 0.3:
        lines.append("")
    if r.random() < 0.6:
        lines.append(indent * (base + 1) + f"APPassword: '{r.choice(CRED_VALUES)}'")
    lines.append(indent * base + r.choice(["system:", "user:", "time_zone: Etc/UTC"]))
    lines.append(indent * (base + 1) + f"root_password: {r.choice(CRED_VALUES)}")
    prefix = r.choice(["config DEBUG CONF:", "CONF:", "x CONF: y CONF:"])
    return [prefix + " " * r.randint(0, 2) + line for line in lines]


def url(r):
    return r.choice(
        [
            f"https://user:{r.choice(CRED_VALUES)}@example.com/path?token={hexstr(r, 16)}&x=1",
            f"http://[fe80::{hexstr(r, 4)}]:8080/api/v1/settings",
            f"https://hawkbit.example.com/DEFAULT/controller/v1/{r.randint(100000, 999999)}",
            f"ws://{ipv4(r)}:8080/socket.io/?EIO=4&transport=websocket&api_key={hexstr(r, 8)}",
        ]
    )


def json_blob(r):
    keys = ["serial", "ssid", "password", "token", "mac", "ip", "time_zone", "hostname", "x"]
    obj = {}
    for key in r.sample(keys, r.randint(1, 5)):
        obj[key] = r.choice(
            [
                r.choice(SSIDS),
                mac(r),
                ipv6(r),
                timezone(r),
                r.randint(0, 10**6),
                r.choice(CRED_VALUES),
                None,
                True,
            ]
        )
    return json.dumps(obj, ensure_ascii=r.random() < 0.5)


def noise(r, n=None):
    n = n if n is not None else r.randint(1, 24)
    return "".join(r.choice(TRICKY) for _ in range(n))


def words(r):
    return " ".join(r.choice(WORDS) for _ in range(r.randint(1, 8)))


def mutate(r, s):
    s = list(s)
    for _ in range(r.randint(1, 4)):
        roll = r.random()
        pos = r.randint(0, len(s))
        if roll < 0.5 or not s:
            s.insert(pos, r.choice(TRICKY))
        elif roll < 0.75:
            del s[min(pos, len(s) - 1)]
        elif roll < 0.9:
            i = min(pos, len(s) - 1)
            s[i] = s[i].swapcase()
        else:
            i = min(pos, len(s) - 1)
            s[i:i] = s[i : i + r.randint(1, 6)]
    return "".join(s)


def message(r, ssids):
    roll = r.random()
    if roll < 0.18:
        msg = ssid_line(r, r.choice(ssids))
    elif roll < 0.33:
        msg = cred_kv(r)
    elif roll < 0.41:
        msg = f"authenticate with {mac(r)} ({words(r)})"
    elif roll < 0.51:
        msg = f"Registering new address record for {ipv6(r)} on wlan0.*."
    elif roll < 0.6:
        msg = r.choice(
            [
                f"systemd-timedated - Changed time zone to '{timezone(r)}",
                f"new system time zone: {timezone(r)}",
                f"Time zone: {timezone(r)}",
                timezone(r),
            ]
        )
    elif roll < 0.65:
        msg = url(r)
    elif roll < 0.7:
        msg = json_blob(r)
    elif roll < 0.73:
        msg = f"contact {email(r)} from {ipv4(r)}"
    elif roll < 0.76:
        msg = (
            f"Serial_number: {r.randint(100000, 999999)} Batch_number: 00{r.randint(0, 99):02d}"
        )
    elif roll < 0.82:
        msg = noise(r)
    elif roll < 0.88:
        msg = " ".join(
            r.choice([mac(r), ipv6(r), timezone(r), r.choice(ssids), cred_kv(r), words(r)])
            for _ in range(r.randint(2, 5))
        )
    else:
        msg = words(r)
    if r.random() < 0.25:
        msg = mutate(r, msg)
    return r.choice(PREFIXES) + msg


# Pieces of the rules' own syntax, glued together at random: dense coverage of
# rule interactions, boundaries and backtracking that realistic lines rarely hit.
FRAGMENTS = [
    "ssid",
    "SSID",
    "Ssid",
    U(0x17F) + "sid",
    "password",
    "root_",
    "ap",
    "psk",
    "token",
    "api",
    "-key",
    "_key",
    "key",
    "secret",
    "bearer",
    "added 'psk' value ",
    "Config: added 'ssid' value ",
    "SSID='",
    "associate with SSID '",
    'wireless network "',
    "access point '",
    "connection '",
    "policy: set '",
    ":",
    "=",
    " ",
    "  ",
    "\t",
    "'",
    '"',
    "#",
    ",",
    ";",
    "}",
    "]",
    "[",
    "::",
    ":",
    "a",
    "f",
    "F",
    "1",
    "12",
    "abcd",
    "ABCDE",
    "fe80",
    "0:0",
    "aa:bb:cc:dd:ee:ff",
    "1:2:3:4:5:6:7",
    "Etc/",
    "America/",
    "US/",
    "Asia/",
    "GMT",
    "UTC",
    "-6",
    "+12",
    "Mexico_City",
    "/x",
    "(CST)",
    "(",
    ")",
    "/*****",
    "*****",
    "CONF:",
    "KnownWifis:",
    "  KnownWifis:",
    " |",
    "{}",
    "\n",
    "\r",
    "\r\n",
    "\x0b",
    "\x1c",
    "\x1f",
    U(0x85),
    U(0x2028),
    "[REDACTED]",
    "[redacted]",
    "[SSID_604cf397]",
    "[MAC_cfe506c4]",
    "[IPV6_4bf39a5b]",
    "_",
    "-",
    ".",
    "x",
    "HomeNet",
    "Home",
    "lo",
    "eth0",
    "Wired connection 1",
    U(0xE9),
    U(0x301),
    U(0x130),
    U(0x131),
    U(0x212A),
    U(0x663),
    U(0xB2),
    U(0x3000),
    U(0xA0),
    U(0x7F51),
    U(0x1F600),
]


def soup(r):
    return "".join(r.choice(FRAGMENTS) for _ in range(r.randint(1, 24)))


def document(r, ssids):
    lines = []
    for _ in range(r.randint(1, 8)):
        if r.random() < 0.08:
            lines.extend(conf_dump(r, ssids))
        else:
            lines.append(message(r, ssids))
    text = ""
    for line in lines:
        text += line + r.choice(LINE_BREAKS)
    if r.random() < 0.4:
        text = text[:-1] if text.endswith(("\n", "\r")) else text
    return text


def key(r):
    roll = r.random()
    if roll < 0.4:
        return bytes(range(32))
    if roll < 0.7:
        return b"\x00" * 32
    if roll < 0.9:
        return bytes(r.randrange(256) for _ in range(32))
    return bytes(r.randrange(256) for _ in range(r.choice([1, 16, 64, 65, 100])))


def generate(seed, scale):
    r = random.Random(seed)
    cases = {"redact": [], "stateful": [], "filter": []}
    for _ in range(2500 * scale):
        ssids = r.sample(SSIDS, r.randint(1, 6))
        cases["redact"].append({"key": key(r).hex(), "text": document(r, ssids)})
    for _ in range(3000 * scale):
        text = "".join(soup(r) + r.choice(LINE_BREAKS) for _ in range(r.randint(1, 4)))
        cases["redact"].append({"key": key(r).hex(), "text": text})
    for _ in range(250 * scale):
        ssids = r.sample(SSIDS, r.randint(1, 6))
        records = []
        while len(records) < r.randint(4, 20):
            roll = r.random()
            if roll < 0.3:
                records.extend(conf_dump(r, ssids))
            elif roll < 0.5:
                records.append(soup(r))
            else:
                records.append(message(r, ssids))
        cases["stateful"].append({"key": key(r).hex(), "records": records})
    for _ in range(300 * scale):
        ssids = r.sample(SSIDS, r.randint(0, 8))
        creds = r.sample(CRED_VALUES, r.randint(0, 5))
        if r.random() < 0.3:
            # Values that are prefixes of each other or full of rule syntax.
            ssids += [soup(r) for _ in range(r.randint(1, 4))]
            creds += [soup(r) for _ in range(r.randint(0, 2))]
        if r.random() < 0.2 and ssids:
            creds.append(r.choice(ssids))
        if r.random() < 0.2 and creds:
            ssids.append(r.choice(creds))
        records = []
        while len(records) < r.randint(3, 15):
            pool = ssids + creds or SSIDS
            roll = r.random()
            if roll < 0.15:
                records.extend(conf_dump(r, pool))
            elif roll < 0.25:
                records.append(soup(r) + r.choice(pool) + soup(r))
            elif roll < 0.6:
                value = r.choice(pool)
                around = r.choice(["", " ", "-", "_", "x", ".", "'", U(0xE9), U(0x301), "\n"])
                records.append(
                    r.choice(PREFIXES)
                    + words(r)
                    + r.choice([" ", ""])
                    + around
                    + value
                    + r.choice(["", " ", "-", "_", "x", ".", "'", U(0xE9), U(0x301), "s"])
                    + r.choice([" ", ""])
                    + words(r)
                )
            else:
                records.append(message(r, pool))
        cases["filter"].append(
            {"key": key(r).hex(), "ssids": ssids, "credentials": creds, "records": records}
        )
    return cases


# --------------------------------------------------------------------------
# running the Python implementation
# --------------------------------------------------------------------------


def compute(cases):
    sys.path.insert(0, REPO)
    import logging

    import log_redaction_filter
    from log_redactor import RedactionState, redact

    out = {"redact": [], "stateful": [], "filter": []}
    for case in cases["redact"]:
        text, learned = redact(case["text"], bytes.fromhex(case["key"]))
        out["redact"].append({"text": text, "learned": sorted(learned)})
    for case in cases["stateful"]:
        state = RedactionState()
        key_bytes = bytes.fromhex(case["key"])
        out["stateful"].append(
            [redact(record, key_bytes, state=state)[0] for record in case["records"]]
        )
    with tempfile.TemporaryDirectory() as tmp:
        for index, case in enumerate(cases["filter"]):
            key_path = os.path.join(tmp, f"key{index}")
            with open(key_path, "wb") as fh:
                fh.write(bytes.fromhex(case["key"]))
            log_redaction_filter.reset_key_cache()
            flt = log_redaction_filter.LogRedactionFilter(key_path=key_path)
            ssids, creds = list(case["ssids"]), list(case["credentials"])
            flt.set_value_provider(lambda: (ssids, creds))
            msgs = []
            for record_text in case["records"]:
                record = logging.LogRecord(
                    "wifi", logging.INFO, __file__, 1, record_text, None, None
                )
                flt.filter(record)
                msgs.append(record.msg)
            out["filter"].append(msgs)
    return out


def run_under_hash_seeds(cases):
    with tempfile.TemporaryDirectory() as tmp:
        in_path = os.path.join(tmp, "cases.json")
        with open(in_path, "w", encoding="utf-8") as fh:
            json.dump(cases, fh)
        results = []
        for hash_seed in HASH_SEEDS:
            out_path = os.path.join(tmp, f"out{hash_seed}.json")
            env = dict(os.environ, PYTHONHASHSEED=hash_seed)
            subprocess.run(
                [sys.executable, os.path.abspath(__file__), "--compute", in_path, out_path],
                check=True,
                env=env,
            )
            with open(out_path, encoding="utf-8") as fh:
                results.append(json.load(fh))
    return results


def write_jsonl(path, rows):
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=int, default=20261024)
    parser.add_argument("--scale", type=int, default=1)
    parser.add_argument("--out", default=DATA)
    parser.add_argument("--compute", nargs=2, metavar=("IN", "OUT"), help=argparse.SUPPRESS)
    args = parser.parse_args()

    if args.compute:
        with open(args.compute[0], encoding="utf-8") as fh:
            cases = json.load(fh)
        with open(args.compute[1], "w", encoding="utf-8") as fh:
            json.dump(compute(cases), fh)
        return

    cases = generate(args.seed, args.scale)
    results = run_under_hash_seeds(cases)
    os.makedirs(args.out, exist_ok=True)
    for kind in ("redact", "stateful", "filter"):
        rows = []
        unstable = 0
        for index, case in enumerate(cases[kind]):
            outputs = [result[kind][index] for result in results]
            if any(output != outputs[0] for output in outputs[1:]):
                unstable += 1
                continue
            rows.append(dict(case, expected=outputs[0]))
        write_jsonl(os.path.join(args.out, f"{kind}_goldens.jsonl"), rows)
        print(f"{kind}: {len(rows)} cases, {unstable} dropped as hash-seed dependent")


if __name__ == "__main__":
    main()
