// Mugenkaid entry point: a Unix-socket JSON-RPC 2.0 server.
//
// Frames are newline-delimited JSON. The daemon supports streaming
// for chat.send: text deltas arrive as JSON-RPC notifications
// (no id) with method "chat.delta", followed by the final response
// with the full content. Non-streaming methods return a single
// response.

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

// ─── request dispatch ────────────────────────────────────

async function handleRequest(
  req: JsonRpcRequest,
  socket: Socket,
): Promise<JsonRpcResponse> {
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
      return await handleChatSend(req, socket);
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

// ─── chat.send with streaming ────────────────────────────

interface ChatMessage {
  role: "user" | "assistant";
  content: string;
}

interface ChatSendParams {
  messages: ChatMessage[];
  model?: string;
  system?: string;
}

async function handleChatSend(
  req: JsonRpcRequest,
  socket: Socket,
): Promise<JsonRpcResponse> {
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
    stream: true,
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

    // Stream SSE events from Anthropic. Each content_block_delta
    // with type "text_delta" contains a chunk of the response.
    // We forward each chunk as a JSON-RPC notification so the
    // host can update the UI token-by-token.
    const fullContent: string[] = [];
    const reader = resp.body?.getReader();
    if (!reader) {
      return {
        jsonrpc: "2.0",
        id: req.id,
        error: { code: -32003, message: "no readable stream from anthropic" },
      };
    }

    const decoder = new TextDecoder();
    let sseBuffer = "";

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      sseBuffer += decoder.decode(value, { stream: true });

      // SSE events are separated by double newlines. Each event
      // has "event: <type>\ndata: <json>\n\n" format.
      const events = sseBuffer.split("\n\n");
      // Keep the last (possibly incomplete) chunk in the buffer.
      sseBuffer = events.pop() ?? "";

      for (const event of events) {
        const dataLine = event
          .split("\n")
          .find((l) => l.startsWith("data: "));
        if (!dataLine) continue;
        const json = dataLine.slice("data: ".length);
        if (json === "[DONE]") continue;

        try {
          const parsed = JSON.parse(json) as {
            type?: string;
            delta?: { type?: string; text?: string };
          };
          if (
            parsed.type === "content_block_delta" &&
            parsed.delta?.type === "text_delta" &&
            typeof parsed.delta.text === "string"
          ) {
            const text = parsed.delta.text;
            fullContent.push(text);

            // Push delta notification to the host — no id means
            // it's a notification per JSON-RPC 2.0.
            const notification = JSON.stringify({
              jsonrpc: "2.0",
              method: "chat.delta",
              params: { text },
            });
            socket.write(notification + "\n");
          }
        } catch {
          // Malformed SSE chunk — skip.
        }
      }
    }

    const content = fullContent.join("");
    return { jsonrpc: "2.0", id: req.id, result: { content } };
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

// ─── connection handler ──────────────────────────────────

async function parseAndDispatch(
  line: string,
  socket: Socket,
): Promise<JsonRpcResponse | null> {
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
  if (req.id === undefined) return null;
  return await handleRequest(req, socket);
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
        void parseAndDispatch(line, socket).then((response) => {
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

// ─── main ────────────────────────────────────────────────

function main() {
  const path = defaultSocketPath();
  const dir = dirname(path);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  if (existsSync(path)) unlinkSync(path);

  const server = createServer(handleConnection);
  server.listen(path, () => {
    console.log(`mugenkaid ${VERSION} listening on ${path}`);
  });
  server.on("error", (err) => {
    console.error(`[mugenkaid] server error: ${err.message}`);
    process.exit(1);
  });

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
