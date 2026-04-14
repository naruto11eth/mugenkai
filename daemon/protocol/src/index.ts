// Mugenkai host ↔ daemon JSON-RPC 2.0 protocol.
//
// Frames are UTF-8 JSON objects separated by `\n`. Each frame is
// either a request (`method` + `id`) or a response (`result` or
// `error` + matching `id`). The daemon is the server; the host is
// the client. Notifications (no `id`) are allowed but unused in
// v0.1.
//
// Keeping the transport newline-delimited makes it trivial to
// inspect from either side with `ncat -U` or `socat`.

export const PROTOCOL_VERSION = "0.1.0-dev";

export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown;
}

export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

export interface JsonRpcSuccessResponse {
  jsonrpc: "2.0";
  id: number | string | null;
  result: unknown;
}

export interface JsonRpcErrorResponse {
  jsonrpc: "2.0";
  id: number | string | null;
  error: {
    code: number;
    message: string;
    data?: unknown;
  };
}

export type JsonRpcResponse = JsonRpcSuccessResponse | JsonRpcErrorResponse;
export type JsonRpcMessage = JsonRpcRequest | JsonRpcNotification | JsonRpcResponse;

// Standard JSON-RPC 2.0 error codes (subset).
export const ERROR_PARSE = -32700;
export const ERROR_INVALID_REQUEST = -32600;
export const ERROR_METHOD_NOT_FOUND = -32601;
export const ERROR_INVALID_PARAMS = -32602;
export const ERROR_INTERNAL = -32603;

/// The default Unix socket path used by the daemon. The host
/// connects to this same path to send requests. Respects
/// `XDG_RUNTIME_DIR` per the freedesktop spec; falls back to
/// `/tmp/mugenkai` so the daemon still runs in containers that
/// don't set the runtime dir.
export function defaultSocketPath(): string {
  const runtime = process.env["XDG_RUNTIME_DIR"] ?? "/tmp";
  return `${runtime}/mugenkai/daemon.sock`;
}
