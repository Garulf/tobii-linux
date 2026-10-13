#!/usr/bin/env python3
"""Run a PowerShell script over SSH without nested shell quoting.

Usage: python tools/albedo.py < script.ps1
This utility executes the supplied script; it is not a read-only sandbox.
"""
import base64
import subprocess
import sys


def run(script: str, host: str = "albedo") -> subprocess.CompletedProcess:
    encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
    return subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host,
         "powershell.exe -NoProfile -NonInteractive -EncodedCommand " + encoded],
        capture_output=True, text=True, timeout=120,
    )


if __name__ == "__main__":
    result = run(sys.stdin.read())
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    sys.exit(result.returncode)
