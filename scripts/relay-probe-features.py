#!/usr/bin/env python3
"""
Refresh install-time feature *inventory* (subfeatures) in /opt/relay/state/features.json.

Preserves enabled/expected flags and core structure; updates:
  - piper_tts.voices, piper_tts.languages (from *.onnx in models_dir)
  - text_translation.language_pairs, from_languages, to_languages (Argos, local venv)

Used by install.sh after enabling features and by `install.sh refresh-features`.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path


def _strip_none(d):
    if isinstance(d, dict):
        return {k: _strip_none(v) for k, v in d.items() if v is not None}
    return d


def scan_piper_models(data: dict, install: str) -> None:
    feats = data.get("features") or {}
    piper = feats.get("piper_tts")
    if not isinstance(piper, dict):
        return
    if not piper.get("enabled"):
        piper["voices"] = []
        piper["languages"] = []
        return

    models_dir = piper.get("models_dir") or os.path.join(install, "lib/piper/models")
    voices = []
    langs: set[str] = set()
    if os.path.isdir(models_dir):
        for name in sorted(os.listdir(models_dir)):
            if not name.endswith(".onnx"):
                continue
            base = name.replace(".onnx", "")
            m = re.match(r"^([a-z]{2}_[A-Z]{2})-(.+?)-(low|medium|high|x_low)$", base)
            if m:
                lang, voice, quality = m.group(1), m.group(2), m.group(3)
            else:
                lang, voice, quality = "unknown", base, "unknown"
            langs.add(lang)
            langs.add(lang.split("_")[0])
            voices.append(
                {
                    "id": base,
                    "language": lang,
                    "voice": voice,
                    "quality": quality,
                    "model_file": name,
                }
            )
    piper["voices"] = voices
    piper["languages"] = sorted(langs)


def scan_argos_pairs(data: dict, install: str) -> None:
    feats = data.get("features") or {}
    tr = feats.get("text_translation")
    if not isinstance(tr, dict):
        return
    if not tr.get("enabled"):
        tr["language_pairs"] = []
        tr["from_languages"] = []
        tr["to_languages"] = []
        return

    venv_py = Path(install) / "lib/argos-venv/bin/python3"
    py = str(venv_py) if venv_py.is_file() else sys.executable

    code = r"""
import json
try:
    from argostranslate import package
except Exception as e:
    print(json.dumps({"error": str(e), "pairs": []}))
    raise SystemExit(0)
pairs = []
try:
    for p in package.get_installed_packages():
        pairs.append({
            "from": getattr(p, "from_code", "") or "",
            "to": getattr(p, "to_code", "") or "",
            "package": getattr(p, "package", "") or "",
        })
except Exception as e:
    print(json.dumps({"error": str(e), "pairs": []}))
    raise SystemExit(0)
from_set = sorted({x["from"] for x in pairs if x.get("from")})
to_set = sorted({x["to"] for x in pairs if x.get("to")})
print(json.dumps({"pairs": pairs, "from_languages": from_set, "to_languages": to_set}))
"""
    try:
        out = subprocess.check_output([py, "-c", code], text=True, timeout=120, stderr=subprocess.DEVNULL)
        payload = json.loads(out.strip() or "{}")
    except (subprocess.CalledProcessError, json.JSONDecodeError, FileNotFoundError, OSError):
        tr["language_pairs"] = []
        tr["from_languages"] = []
        tr["to_languages"] = []
        tr["probe_error"] = "argos_probe_failed"
        return

    if payload.get("error"):
        tr["language_pairs"] = []
        tr["from_languages"] = []
        tr["to_languages"] = []
        tr["probe_error"] = str(payload.get("error"))
        return

    tr["language_pairs"] = payload.get("pairs") or []
    tr["from_languages"] = payload.get("from_languages") or []
    tr["to_languages"] = payload.get("to_languages") or []
    tr.pop("probe_error", None)


def merge_inventory(install_root: str) -> None:
    install_root = os.path.abspath(install_root)
    path = Path(install_root) / "state/features.json"
    if not path.is_file():
        print(f"[relay-probe-features] skip: no {path}", file=sys.stderr)
        return
    data = json.loads(path.read_text(encoding="utf-8"))
    scan_piper_models(data, install_root)
    scan_argos_pairs(data, install_root)
    path.write_text(json.dumps(_strip_none(data), indent=2), encoding="utf-8")
    try:
        import pwd

        relay = pwd.getpwnam("relay")
        os.chown(path, relay.pw_uid, relay.pw_gid)
    except (ImportError, KeyError, OSError):
        pass


def main() -> None:
    p = argparse.ArgumentParser(description="Refresh relay features.json inventories")
    p.add_argument("command", choices=["merge"], help="merge: rescan subfeatures into features.json")
    p.add_argument("install_root", help="RELAY install root (e.g. /opt/relay)")
    args = p.parse_args()
    if args.command == "merge":
        merge_inventory(args.install_root)


if __name__ == "__main__":
    main()
