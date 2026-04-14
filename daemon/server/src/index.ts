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

async function handleRequest(req: JsonRpcRequest): Promise<JsonRpcResponse> {
  switch (req.method) {
    case "ping":
      return { jsonrpc: "2.0", id: req.id, result: "pong" };
    case "version":
      return {
        jsonrpc: "2.0",
        id: req.id,
        result: { daemon: VERSION, protocol: PROTOCOL_VERSION },
      };
    case "chat.send":
      return await handleChatSend(req);
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

interface ChatMessage {
  role: "user" | "assistant";
  content: string;
}

interface ChatSendParams {
  messages: ChatMessage[];
  model?: string;
  system?: string;
}

/// Call Anthropic's /v1/messages endpoint and return the first
/// assistant text block.
///
/// The daemon picks up `ANTHROPIC_API_KEY` from its environment.
/// Host-ui spawns it with inherit-parent-env, so whatever shell
/// launched mugenkai sets the key.
async function handleChatSend(req: JsonRpcRequest): Promise<JsonRpcResponse> {
  const apiKey = process.env["ANTHROPIC_API_KEY"];
  if (!apiKey) {
    return {
      jsonrpc: "2.0",
      id: req.id,
      error: {
        code: -32001,
        message: "ANTHROPIC_API_KEY not set in the daemon's environment",
      },
    };
  }

  const params = req.params as ChatSendParams | undefined;
  if (!params || !Array.isArray(params.messages) || params.messages.length === 0) {
    return {
      jsonrpc: "2.0",
      id: req.id,
      error: { code: -32602, message: "chat.send requires params.messages" },
    };
  }

  const model = params.model ?? "claude-sonnet-4-6";
  const body = {
    model,
    max_tokens: 2048,
    system:
      params.system ??
      "You are a helpful assistant embedded in mugenkai, a Linux terminal emulator. Keep responses terse and actionable — the user is a developer at a shell prompt.",
    messages: params.messages,
  };

  try {
    const resp = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-api-key": apiKey,
        "anthropic-version": "2023-06-01",
      },
      body: JSON.stringify(body),
    });
    if (!resp.ok) {
      const text = await resp.text();
      return {
        jsonrpc: "2.0",
        id: req.id,
        error: {
          code: -32002,
          message: `anthropic api ${resp.status}`,
          data: text,
        },
      };
    }
    const payload = (await resp.json()) as {
      content?: Array<{ type: string; text?: string }>;
    };
    const text =
      payload.content
        ?.filter((block) => block.type === "text" && typeof block.text === "string")
        .map((block) => block.text)
        .join("\n") ?? "";
    return { jsonrpc: "2.0", id: req.id, result: { content: text } };
  } catch (err) {
    return {
      jsonrpc: "2.0",
      id: req.id,
      error: {
        code: -32003,
        message: `fetch failed: ${(err as Error).message}`,
      },
    };
  }
}

async function parseAndDispatch(line: string): Promise<JsonRpcResponse | null> {
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
  return await handleRequest(req);
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
        // Fire the async handler and write its response when
        // ready. Connections stay open while requests are in
        // flight — Anthropic API calls can take several seconds.
        void parseAndDispatch(line).then((response) => {
          if (response !== null) {
            socket.write(JSON.stringify(response) + "\n");
          }
        });
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
