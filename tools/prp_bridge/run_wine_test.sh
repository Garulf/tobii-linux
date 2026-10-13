#!/usr/bin/env bash
# End-to-end Wine check: Linux PRP service (synthetic head pose), bridge under Wine,
# and the Stream Engine test client under Wine with an explicit tobii-prp:// URL.
#
# Usage: run_wine_test.sh <stream engine dll> [seconds] [output dir]
# Expects prp_bridge.exe and prp_test_client.exe beside this script and the
# tobii_linux package in ../../src (or PRP_SRC). Uses WINEPREFIX from the environment;
# PRP_DEBUG=1 logs every relayed message in service.log; PRP_RUNS=n runs the client
# n times against the same service and bridge; PRP_URL=discover uses the DLL's own
# pipe discovery instead of the explicit URL.
set -u
here=$(cd "$(dirname "$0")" && pwd)
dll=${1:?stream engine dll path}
seconds=${2:-5}
out=${3:-/tmp/prp-wine-test}
src=$(cd "${PRP_SRC:-$here/../../src}" && pwd)
port=${PRP_PORT:-7760}
name=${PRP_NAME:-ET5-LINUX}
export WINEDEBUG=${WINEDEBUG:--all}
mkdir -p "$out"

(cd "$out" && PYTHONPATH=$src exec python3 -m tobii_linux.prp_service --synthetic \
    --port "$port" --name "$name" ${PRP_DEBUG:+--debug}) > "$out/service.log" 2>&1 &
service=$!
bridge=
trap 'kill $service 2>/dev/null; kill $bridge 2>/dev/null; wineserver -k 2>/dev/null' EXIT
sleep 1

wine "$here/prp_bridge.exe" 127.0.0.1 "$port" > "$out/bridge.log" 2>&1 &
bridge=$!
for _ in $(seq 50); do
    grep -q 'serving' "$out/bridge.log" 2>/dev/null && break
    sleep 0.2
done
sleep 1

: > "$out/client.txt"
for run in $(seq "${PRP_RUNS:-1}"); do
    echo "client run $run" >> "$out/client.txt"
    timeout $((seconds + 60)) wine "$here/prp_test_client.exe" "$(winepath -w "$dll")" \
        "${PRP_URL:-tobii-prp://$name}" "$seconds" 1 >> "$out/client.txt" 2>&1
    echo "client exit $?" >> "$out/client.txt"
done
sleep 0.5
for file in service.log bridge.log client.txt; do
    echo "== $file"
    cat "$out/$file"
done
