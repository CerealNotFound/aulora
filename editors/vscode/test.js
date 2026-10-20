// Tests for the extension with a fake `vscode` module and a fake daemon.
// Run: node editors/vscode/test.js
"use strict";
const assert = require("assert");
const net = require("net");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { create, socketPath, keystrokes, paneTerminal } = require("./extension.js");

function fakeVscode(pids) {
  const listeners = { open: [], close: [] };
  const terminals = pids.map((pid, i) => ({ name: `zsh ${i}`, processId: Promise.resolve(pid), sent: [], shown: false, sendText(t) { this.sent.push(t); }, show() { this.shown = true; } }));
  return {
    terminals,
    window: {
      terminals,
      onDidOpenTerminal: (f) => (listeners.open.push(f), { dispose() {} }),
      onDidCloseTerminal: (f) => (listeners.close.push(f), { dispose() {} }),
    },
    env: { appName: "Visual Studio Code" },
    open(t) { terminals.push(t); listeners.open.forEach((f) => f(t)); },
  };
}

function lines(sock, onMsg) {
  let buf = "";
  sock.setEncoding("utf8");
  sock.on("data", (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      onMsg(JSON.parse(buf.slice(0, i)));
      buf = buf.slice(i + 1);
    }
  });
}

const until = async (f, ms = 3000) => {
  const end = Date.now() + ms;
  while (!f()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 10));
  }
};

async function main() {
  // The socket path matches aulora's, including the short fallback.
  assert.strictEqual(socketPath({ AULORA_HOME: "/x" }), "/x/aulora.sock");
  assert.match(socketPath({ AULORA_HOME: "/" + "d".repeat(120) }), /^\/tmp\/aulora-[0-9a-f]{8}\.sock$/);
  assert.strictEqual(keystrokes("hi"), "hi");
  assert.strictEqual(keystrokes("a\nb"), "\x1b[200~a\nb\x1b[201~");

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "aulora-ext-"));
  const sock = path.join(dir, "s.sock");
  const got = [];
  let client = null;
  const server = net.createServer((c) => {
    client = c;
    lines(c, (m) => got.push(m));
  });
  await new Promise((r) => server.listen(sock, r));

  const vscode = fakeVscode([111, 222]);
  const bridge = create(vscode, { socket: sock, retryMs: 20 });
  await until(() => got.some((m) => m.type === "editor"));
  const hello = got.find((m) => m.type === "editor");
  assert.strictEqual(hello.app, "Visual Studio Code");
  assert.deepStrictEqual(hello.terminals.map((t) => t.pid), [111, 222]);

  // Typing: text, then Enter, then an answer.
  const id = hello.terminals[1].id;
  client.write(JSON.stringify({ type: "type", request_id: 9, terminal_id: id, text: "fix the tests" }) + "\n");
  await until(() => got.some((m) => m.type === "typed"));
  assert.deepStrictEqual(vscode.terminals[1].sent, ["fix the tests", "\r"]);
  assert.deepStrictEqual(got.find((m) => m.type === "typed"), { type: "typed", request_id: 9, error: null });

  // Answer keys go as they are, without Enter.
  client.write(JSON.stringify({ type: "type", request_id: 11, terminal_id: id, text: "2", enter: false }) + "\n");
  await until(() => got.some((m) => m.request_id === 11));
  assert.deepStrictEqual(vscode.terminals[1].sent.slice(-1), ["2"]);

  // Showing a terminal brings it into view.
  client.write(JSON.stringify({ type: "show", terminal_id: id }) + "\n");
  await until(() => vscode.terminals[1].shown);

  // An unknown terminal is an error, not a crash.
  client.write(JSON.stringify({ type: "type", request_id: 10, terminal_id: 99, text: "x" }) + "\n");
  await until(() => got.some((m) => m.request_id === 10));
  assert.ok(got.find((m) => m.request_id === 10).error);

  // New terminals are announced.
  vscode.open({ name: "zsh 2", processId: Promise.resolve(333), sent: [], sendText() {} });
  await until(() => got.filter((m) => m.type === "editor").some((m) => m.terminals.length === 3));

  // It reconnects when the daemon restarts.
  client.destroy();
  got.length = 0;
  await until(() => got.some((m) => m.type === "editor"));

  bridge.dispose();

  // A pane as a terminal: attach, screen in, keys and size out.
  class Emitter { constructor() { this.l = []; this.event = (f) => (this.l.push(f), { dispose() {} }); } fire(x) { this.l.forEach((f) => f(x)); } }
  const fake = { EventEmitter: Emitter, window: { createTerminal: (opts) => opts } };
  const paneMsgs = [];
  let paneClient = null;
  const paneSock = path.join(dir, "p.sock");
  const paneServer = net.createServer((c) => {
    paneClient = c;
    lines(c, (m) => {
      paneMsgs.push(m);
      if (m.type === "attach") c.write(JSON.stringify({ type: "pane_screen", pane_id: "p1", cols: 80, rows: 20, data: Buffer.from("hello").toString("base64") }) + "\n");
    });
  });
  await new Promise((r) => paneServer.listen(paneSock, r));
  const term = paneTerminal(fake, paneSock, { id: "p1", name: "shell" });
  let shown = "";
  term.pty.onDidWrite((d) => (shown += d));
  term.pty.open({ columns: 80, rows: 20 });
  await until(() => shown.includes("hello"));
  term.pty.handleInput("ls\r");
  term.pty.setDimensions({ columns: 100, rows: 30 });
  await until(() => paneMsgs.some((m) => m.type === "resize" && m.cols === 100));
  assert.strictEqual(term.name, "aulora · shell");
  assert.ok(paneMsgs.some((m) => m.type === "input" && Buffer.from(m.data, "base64").toString() === "ls\r"));
  term.pty.close();
  await until(() => paneMsgs.some((m) => m.type === "detach"));
  paneServer.close();
  if (paneClient) paneClient.destroy();

  server.close();
  fs.rmSync(dir, { recursive: true, force: true });
  console.log("extension tests passed");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
