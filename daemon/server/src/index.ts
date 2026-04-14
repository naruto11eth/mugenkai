// Mugenkaid entry point: a Unix-socket JSON-RPC 2.0 server.
//
// Phase 1: accepts connections on the configured socket path
// and responds to a single handshake method (`ping` → "pong"),
// plus `version` → the daemon version. Everything else returns
// METHOD_NOT_FOUND. Phase 3 adds real skill dispatch.
//
// Frames are newline-delimited JSON. One client → one request →
// one response. No streaming, no batching — keeping the wire
// format simple so the Rust client can be built on `BufReader`
// + `writeln!` with no framing library.

import { createServer, type Socket } from "node:net";
import { existsSync, mkdirSync, unlinkSync } from "node:fs";
import { dirname } from "node:path";

import {
  PROTOCOL_VERSION,
  defaultSocketPath,
  ERROR_METHOD_NOT_FOUND,
  ERROR_PARSE,
  ERROR_INVALID_REQUEST,
  type JsonRpcRequest,
  type JsonRpcResponse,
} from "@mugenkai/daemon-protocol";

const VERSION = "0.1.0-dev";

function handleRequest(req: JsonRpcRequest): JsonRpcResponse {
  switch (req.method) {
    case "ping":
      return { jsonrpc: "2.0", id: req.id, result: "pong" };
    case "version":
      return {
        jsonrpc: "2.0",
        id: req.id,
        result: { daemon: VERSION, protocol: PROTOCOL_VERSION },
      };
    default:
      return {
        jsonrpc: "2.0",
        id: req.id,
        error: {
          code: ERROR_METHOD_NOT_FOUND,
          message: `method not found: ${req.method}`,
        },
      };
  }
}

function parseAndDispatch(line: string): JsonRpcResponse | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return {
      jsonrpc: "2.0",
      id: null,
      error: { code: ERROR_PARSE, message: "parse error" },
    };
  }
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    (parsed as JsonRpcRequest).jsonrpc !== "2.0"
  ) {
    return {
      jsonrpc: "2.0",
      id: null,
      error: { code: ERROR_INVALID_REQUEST, message: "invalid jsonrpc envelope" },
    };
  }
  const req = parsed as JsonRpcRequest;
  // Notifications (no id) get no response.
  if (req.id === undefined) return null;
  return handleRequest(req);
}

function handleConnection(socket: Socket) {
  let buffer = "";
  socket.on("data", (chunk) => {
    buffer += chunk.toString("utf8");
    let newlineIdx = buffer.indexOf("\n");
    while (newlineIdx >= 0) {
      const line = buffer.slice(0, newlineIdx);
      buffer = buffer.slice(newlineIdx + 1);
      if (line.length > 0) {
        const response = parseAndDispatch(line);
        if (response !== null) {
          socket.write(JSON.stringify(response) + "\n");
        }
      }
      newlineIdx = buffer.indexOf("\n");
    }
  });
  socket.on("error", (err) => {
    console.error(`[mugenkaid] socket error: ${err.message}`);
  });
}

function main() {
  const path = defaultSocketPath();
  const dir = dirname(path);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  // Stale socket from a previous run blocks `bind` with EADDRINUSE.
  if (existsSync(path)) unlinkSync(path);

  const server = createServer(handleConnection);
  server.listen(path, () => {
    console.log(`mugenkaid ${VERSION} listening on ${path}`);
  });
  server.on("error", (err) => {
    console.error(`[mugenkaid] server error: ${err.message}`);
    process.exit(1);
  });

  // Clean up on shutdown so the next run doesn't trip on a stale
  // socket if we crashed before the pre-listen unlink.
  const shutdown = () => {
    try {
      server.close();
      if (existsSync(path)) unlinkSync(path);
    } catch {}
    process.exit(0);
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

main();
