#!/usr/bin/env python3
"""Persistent REA MCP client for local research; JSON requests on stdin.

Each input: {"name": "tool_name", "arguments": {}, "output": "path.json"}.
Use name="tools/list" for schemas. Complete tool responses go to output files.
Pass the REA executable as argv[1]. Provider configuration is inherited.
"""
import json
import pathlib
import subprocess
import sys


def main():
    process = subprocess.Popen([sys.argv[1], "mcp"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=sys.stderr, text=True)
    request_id = 0

    def rpc(method, params):
        nonlocal request_id
        request_id += 1
        process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=request_id,
                                           method=method, params=params)) + "\n")
        process.stdin.flush()
        for line in process.stdout:
            msg = json.loads(line)
            if msg.get("id") == request_id:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg["result"]
        raise RuntimeError("REA exited before responding")

    try:
        rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                           "clientInfo": {"name": "tobii-research", "version": "0.1"}})
        process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        process.stdin.flush()
        print("REA session ready", flush=True)
        for line in sys.stdin:
            try:
                command = json.loads(line)
                name = command["name"]
                result = rpc("tools/list", {}) if name == "tools/list" else rpc(
                    "tools/call", {"name": name, "arguments": command.get("arguments", {})})
                path = pathlib.Path(command["output"])
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(result, indent=2) + "\n")
                print(json.dumps({"output": str(path), "isError": result.get("isError", False)}), flush=True)
            except Exception as error:
                print(json.dumps({"error": str(error)}), flush=True)
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=15)


if __name__ == "__main__":
    main()
