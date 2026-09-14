#!/usr/bin/env python3
"""Paranoid static-analysis orchestrator for VSRO drops.

Runs inside the sro-safe-analyze container with no network access.
Reads input from /work/input (read-only mount), writes JSON verdict +
extracted files to /work/output. Tools used:

    file       — magic-byte type detection
    7z / unzip — extraction
    sha256sum  — per-file hashing
    clamscan   — known-malware signatures
    yara       — pattern-based detection (rules at /opt/rules.yar)
    binwalk    — embedded artefact / overlay detection
    pefile     — Windows PE import-table + section entropy
    strings    — suspicious-string heuristics (URLs, miners, etc.)

Verdict levels:
    clean       — no signals
    suspicious  — heuristic / YARA hits, no AV hit
    infected    — ClamAV signature match
    error       — analysis itself failed (e.g. corrupt archive)
"""

import hashlib
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path

INPUT_DIR = Path("/work/input")
OUTPUT_DIR = Path("/work/output")
EXTRACT_DIR = OUTPUT_DIR / "extracted"
REPORT_PATH = OUTPUT_DIR / "report.json"

ARCHIVE_EXTS = {".zip", ".7z", ".rar", ".tar", ".gz", ".tgz", ".bz2", ".xz"}
PE_EXTS = {".exe", ".dll", ".sys", ".ocx", ".scr", ".cpl"}

SUSPICIOUS_STRING_PATTERNS = [
    # C2-style
    (r"https?://(?:[a-z0-9-]+\.)+(?:top|xyz|tk|ml|ga|cf|gq|pw|click)\b", "suspicious-tld-url"),
    (r"https?://\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}(?::\d+)?", "hardcoded-ip-url"),
    # Miners
    (r"(?i)xmrig|monero|stratum\+tcp", "miner-indicator"),
    # Exfil
    (r"discord(?:app)?\.com/api/webhooks/", "discord-webhook"),
    (r"api\.telegram\.org/bot[0-9]+:", "telegram-bot"),
    # Shells
    (r"(?i)powershell.*(?:DownloadString|DownloadFile|IEX|Invoke-Expression)", "powershell-cradle"),
    (r"(?i)cmd\.exe\s*/c", "cmd-execution"),
    # Persistence
    (r"Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Run", "registry-autorun"),
    (r"schtasks\s+/create", "scheduled-task"),
    # Tor
    (r"[a-z2-7]{56}\.onion", "tor-onion"),
    # Ransomware
    (r"vssadmin\s+delete\s+shadows", "shadow-copy-deletion"),
    (r"(?i)bitcoin:[13a-z][a-z0-9]+", "bitcoin-uri"),
]

SUSPICIOUS_IMPORTS = {
    # Process injection
    "VirtualAllocEx", "WriteProcessMemory", "CreateRemoteThread",
    "NtUnmapViewOfSection", "QueueUserAPC", "SetThreadContext",
    # Keylogger
    "SetWindowsHookExA", "SetWindowsHookExW", "GetAsyncKeyState",
    # Anti-analysis
    "IsDebuggerPresent", "CheckRemoteDebuggerPresent",
    "NtQueryInformationProcess", "OutputDebugStringA",
    # Privilege escalation
    "AdjustTokenPrivileges", "OpenProcessToken",
    # Crypto (ransomware)
    "CryptEncrypt", "CryptGenKey", "CryptAcquireContextA",
}


def sh(cmd, **kw):
    """Run a shell command, capture output, never raise on non-zero."""
    try:
        r = subprocess.run(
            cmd, shell=isinstance(cmd, str), capture_output=True,
            text=True, errors="replace", timeout=kw.get("timeout", 600),
        )
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"
    except Exception as e:
        return 1, "", str(e)


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def entropy(data):
    if not data:
        return 0.0
    counts = [0] * 256
    for b in data:
        counts[b] += 1
    n = len(data)
    e = 0.0
    for c in counts:
        if c:
            p = c / n
            e -= p * math.log2(p)
    return round(e, 3)


def detect_archive(path):
    return path.suffix.lower() in ARCHIVE_EXTS


def extract_archive(path, dest):
    dest.mkdir(parents=True, exist_ok=True)
    ext = path.suffix.lower()
    if ext in {".zip", ".7z", ".rar"}:
        rc, _, err = sh(["7z", "x", "-y", f"-o{dest}", str(path)])
    elif ext in {".tar", ".gz", ".tgz", ".bz2", ".xz"}:
        rc, _, err = sh(["tar", "-xf", str(path), "-C", str(dest)])
    else:
        return False, "unknown archive extension"
    return rc == 0, err if rc != 0 else ""


def walk_files(root):
    for dirpath, _, files in os.walk(root):
        for f in files:
            yield Path(dirpath) / f


def clamav_scan(target):
    # Returns list of {path, sig}. Skip-on-missing-sigs = warn, not fail.
    sig_paths = ["/var/lib/clamav/main.cvd", "/var/lib/clamav/main.cld"]
    if not any(Path(p).exists() for p in sig_paths):
        return {"available": False, "reason": "no signatures in image"}, []

    rc, out, _ = sh(["clamscan", "-r", "--no-summary", "--infected", str(target)])
    hits = []
    for line in out.splitlines():
        if ": " in line and line.strip().endswith("FOUND"):
            p, rest = line.rsplit(": ", 1)
            hits.append({"path": p, "signature": rest.replace(" FOUND", "")})
    return {"available": True, "exit": rc}, hits


def yara_scan(target):
    if not Path("/opt/rules.yar").exists():
        return []
    rc, out, _ = sh(["yara", "-r", "-s", "/opt/rules.yar", str(target)])
    hits = []
    for line in out.splitlines():
        # YARA -s output: "rule_name path" then "0x...: $a: matched_string"
        if line and not line.startswith("0x") and " " in line:
            rule, _, path = line.partition(" ")
            hits.append({"rule": rule, "path": path})
    return hits


def binwalk_scan(path):
    rc, out, _ = sh(["binwalk", "-q", str(path)])
    findings = []
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[0].isdigit():
            findings.append({"offset": parts[0], "hex": parts[1], "desc": parts[2]})
    return findings


def pe_analyze(path):
    try:
        import pefile
        pe = pefile.PE(str(path), fast_load=True)
        pe.parse_data_directories(directories=[
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]
        ])
        info = {
            "machine": hex(pe.FILE_HEADER.Machine),
            "is_dll": pe.is_dll(),
            "is_exe": pe.is_exe(),
            "timestamp": pe.FILE_HEADER.TimeDateStamp,
            "sections": [],
            "suspicious_imports": [],
            "imports_total": 0,
        }
        for s in pe.sections:
            name = s.Name.rstrip(b"\x00").decode("ascii", errors="replace")
            data = s.get_data()
            info["sections"].append({
                "name": name,
                "vsize": s.Misc_VirtualSize,
                "rsize": s.SizeOfRawData,
                "entropy": entropy(data),
                "writable_executable": bool(s.IMAGE_SCN_MEM_EXECUTE and s.IMAGE_SCN_MEM_WRITE),
            })
        if hasattr(pe, "DIRECTORY_ENTRY_IMPORT"):
            for entry in pe.DIRECTORY_ENTRY_IMPORT:
                for imp in entry.imports:
                    info["imports_total"] += 1
                    if imp.name:
                        n = imp.name.decode("ascii", errors="replace")
                        if n in SUSPICIOUS_IMPORTS:
                            info["suspicious_imports"].append({
                                "dll": entry.dll.decode("ascii", errors="replace"),
                                "func": n,
                            })
        return info
    except Exception as e:
        return {"error": str(e)}


def suspicious_strings(path, max_hits=50):
    rc, out, _ = sh(["strings", "-a", "-n", "8", str(path)])
    hits = []
    for line in out.splitlines():
        for pat, tag in SUSPICIOUS_STRING_PATTERNS:
            if re.search(pat, line):
                hits.append({"tag": tag, "string": line[:200]})
                if len(hits) >= max_hits:
                    return hits
    return hits


def analyze_input(input_path):
    is_dir = input_path.is_dir()
    report = {
        "input": str(input_path.relative_to(INPUT_DIR)),
        "size": input_path.stat().st_size,
        "sha256": "" if is_dir else sha256_file(input_path),
        "file_type": "",
        "extracted": False,
        "extract_error": None,
        "files": [],
        "clamav": {},
        "yara_hits": [],
        "verdict": "clean",
        "verdict_reasons": [],
    }

    if is_dir:
        report["file_type"] = "(directory)"
    else:
        rc, out, _ = sh(["file", "-b", str(input_path)])
        report["file_type"] = out.strip() if rc == 0 else "(file detection failed)"

    scan_root = input_path
    if not is_dir and detect_archive(input_path):
        ok, err = extract_archive(input_path, EXTRACT_DIR)
        report["extracted"] = ok
        if ok:
            scan_root = EXTRACT_DIR
        else:
            report["extract_error"] = err
            report["verdict_reasons"].append("extract-failed")

    clam_meta, clam_hits = clamav_scan(scan_root)
    report["clamav"] = {"meta": clam_meta, "hits": clam_hits}
    if clam_hits:
        report["verdict"] = "infected"
        report["verdict_reasons"].append(f"clamav:{len(clam_hits)}-hits")

    report["yara_hits"] = yara_scan(scan_root)
    if report["yara_hits"] and report["verdict"] != "infected":
        report["verdict"] = "suspicious"
        rules = sorted({h["rule"] for h in report["yara_hits"]})
        report["verdict_reasons"].append(f"yara:{','.join(rules)}")

    # Per-file deep inspection
    if scan_root.is_dir():
        targets = list(walk_files(scan_root))
    else:
        targets = [scan_root]

    for f in targets:
        try:
            rel = str(f.relative_to(scan_root))
        except ValueError:
            rel = str(f)
        entry = {
            "path": rel,
            "size": f.stat().st_size,
            "sha256": sha256_file(f),
        }
        rc, ftype, _ = sh(["file", "-b", str(f)])
        entry["file_type"] = ftype.strip()

        ext = f.suffix.lower()
        is_pe = ext in PE_EXTS or "PE32" in entry["file_type"] or "MS-DOS executable" in entry["file_type"]

        if is_pe:
            entry["pe"] = pe_analyze(f)
            entry["binwalk"] = binwalk_scan(f)
            entry["suspicious_strings"] = suspicious_strings(f)

            pe = entry["pe"]
            if isinstance(pe, dict) and not pe.get("error"):
                if any(s.get("writable_executable") for s in pe.get("sections", [])):
                    report["verdict_reasons"].append(f"{rel}:w+x-section")
                    if report["verdict"] == "clean":
                        report["verdict"] = "suspicious"
                packed = any(s["entropy"] > 7.5 for s in pe.get("sections", []))
                if packed:
                    report["verdict_reasons"].append(f"{rel}:high-entropy")
                if pe.get("suspicious_imports"):
                    report["verdict_reasons"].append(
                        f"{rel}:imports:{len(pe['suspicious_imports'])}"
                    )
                    if report["verdict"] == "clean":
                        report["verdict"] = "suspicious"

            if entry.get("suspicious_strings"):
                tags = sorted({s["tag"] for s in entry["suspicious_strings"]})
                report["verdict_reasons"].append(f"{rel}:strings:{','.join(tags)}")
                if report["verdict"] == "clean":
                    report["verdict"] = "suspicious"

        report["files"].append(entry)

    report["verdict_reasons"] = sorted(set(report["verdict_reasons"]))
    return report


def main():
    if not INPUT_DIR.exists():
        print(f"ERROR: {INPUT_DIR} not mounted", file=sys.stderr)
        sys.exit(2)
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    inputs = list(INPUT_DIR.iterdir())
    if not inputs:
        print(f"ERROR: nothing in {INPUT_DIR}", file=sys.stderr)
        sys.exit(2)

    if len(inputs) > 1:
        print(f"WARN: {len(inputs)} files in /work/input; analyzing first only: {inputs[0].name}",
              file=sys.stderr)

    target = inputs[0]
    print(f"[safe-analyze] analyzing {target.name}", file=sys.stderr)

    try:
        report = analyze_input(target)
    except Exception as e:
        report = {
            "input": target.name,
            "verdict": "error",
            "verdict_reasons": [f"orchestrator-exception:{e}"],
        }

    REPORT_PATH.write_text(json.dumps(report, indent=2, sort_keys=True))
    print(f"[safe-analyze] verdict: {report.get('verdict')}", file=sys.stderr)
    print(f"[safe-analyze] reasons: {report.get('verdict_reasons')}", file=sys.stderr)
    print(f"[safe-analyze] report: {REPORT_PATH}", file=sys.stderr)

    # Exit code reflects verdict so callers can branch.
    sys.exit({"clean": 0, "suspicious": 10, "infected": 20, "error": 30}.get(
        report.get("verdict"), 40
    ))


if __name__ == "__main__":
    main()
