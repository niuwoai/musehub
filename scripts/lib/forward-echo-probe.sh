#!/usr/bin/env bash
# Forward-channel proof: require nonce echo roundtrip (connect-only is insufficient).
set -euo pipefail

# Starts echo target in background; writes bound port to log_file. Echoes port to stdout.
forward_echo_start_target() {
  local log_file="$1"
  python3 - "$log_file" <<'PY' &
import socket, sys
log = sys.argv[1]
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 0))
srv.listen(1)
port = srv.getsockname()[1]
with open(log, "w", encoding="utf-8") as f:
    f.write(str(port))
srv.settimeout(25.0)
try:
    conn, _ = srv.accept()
    conn.settimeout(2.0)
    data = conn.recv(64)
    if data:
        conn.sendall(data)
    conn.close()
except OSError:
    pass
finally:
    srv.close()
PY
  echo $!
}

forward_echo_probe() {
  local local_port="$1"
  local nonce="${2:-musehub-probe-$RANDOM}"
  command -v python3 >/dev/null || return 2
  python3 - "$local_port" "$nonce" <<'PY'
import socket, sys
local_port, nonce = int(sys.argv[1]), sys.argv[2].encode()
s = socket.create_connection(("127.0.0.1", local_port), timeout=0.8)
s.settimeout(2.0)
s.sendall(nonce)
got = s.recv(len(nonce))
s.close()
raise SystemExit(0 if got == nonce else 1)
PY
}
