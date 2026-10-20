// @ts-check
// aulora for VS Code: lets the aulora dashboard type prompts into the
// Claude Code sessions running in this window's terminals.
//
// The extension connects to the aulora daemon's Unix socket, tells it which
// terminals this window has (with their shell PIDs, which the daemon
// matches against each Claude process's parents), and types what the
// daemon asks into the right terminal with `terminal.sendText`. It never
// starts the daemon and stays quiet while it isn't running.
"use strict";

const net = require("net");
const os = require("os");
const path = require("path");

/** The daemon's socket, computed exactly as aulora does (paths.rs). */
function socketPath(env = process.env) {
  const home = env.AULORA_HOME && env.AULORA_HOME.length > 0 ? env.AULORA_HOME : path.join(os.homedir(), ".aulora");
  const p = path.join(home, "aulora.sock");
  if (Buffer.byteLength(p) <= 100) return p;
  // FNV-1a over the home path, as in paths::socket().
  let h = 0x811c9dc5;
  for (const b of Buffer.from(home)) {
    h ^= b;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return `/tmp/aulora-${h.toString(16).padStart(8, "0")}.sock`;
}

/** Text as keystrokes: a paste for multi-line text, so it arrives whole. */
function keystrokes(text) {
  return text.includes("\n") ? `\x1b[200~${text}\x1b[201~` : text;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * @param {any} vscode the `vscode` module (injected, so tests can fake it)
 * @param {{ socket?: string, log?: (msg: string) => void, retryMs?: number }} [opts]
 */
function create(vscode, opts = {}) {
  const log = opts.log || (() => {});
  const socket = opts.socket || socketPath();
  /** @type {Map<any, number>} */
  const ids = new Map();
  let nextId = 1;
  /** @type {import("net").Socket | null} */
  let conn = null;
  let buffer = "";
  let retry = opts.retryMs || 1000;
  let timer = null;
  let stopped = false;
  let announceTimer = null;

  const idFor = (t) => {
    if (!ids.has(t)) ids.set(t, nextId++);
    return /** @type {number} */ (ids.get(t));
  };
  const terminalById = (id) => {
    for (const [t, i] of ids) if (i === id) return t;
    return undefined;
  };

  async function terminals() {
    const list = [];
    for (const t of vscode.window.terminals) {
      const pid = await t.processId;
      if (typeof pid === "number") list.push({ id: idFor(t), pid, name: t.name || "" });
    }
    return list;
  }

  function send(msg) {
    if (conn && !conn.destroyed) conn.write(JSON.stringify(msg) + "\n");
  }

  async function announce() {
    send({ type: "editor", app: vscode.env.appName || "VS Code", terminals: await terminals() });
  }

  /** Tell the daemon soon, once a burst of terminal changes settles. */
  function announceSoon() {
    clearTimeout(announceTimer);
    announceTimer = setTimeout(() => announce().catch((e) => log(`announce: ${e}`)), 100);
  }

  async function onType(msg) {
    const t = terminalById(msg.terminal_id);
    if (!t || !vscode.window.terminals.includes(t)) {
      send({ type: "typed", request_id: msg.request_id, error: "that terminal is closed" });
      return;
    }
    try {
      t.sendText(keystrokes(msg.text), false);
      // Let the program take in the text before Enter submits it.
      await sleep(msg.text.includes("\n") ? 150 : 50);
      t.sendText("\r", false);
      send({ type: "typed", request_id: msg.request_id, error: null });
    } catch (e) {
      send({ type: "typed", request_id: msg.request_id, error: String(e) });
    }
  }

  function onLine(line) {
    if (!line.trim()) return;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg.type === "type") onType(msg);
    if (msg.type === "show") {
      const t = terminalById(msg.terminal_id);
      if (t) t.show(false);
    }
  }

  function connect() {
    if (stopped) return;
    const c = net.createConnection(socket);
    conn = c;
    c.setEncoding("utf8");
    c.on("connect", () => {
      log(`connected to ${socket}`);
      retry = opts.retryMs || 1000;
      announce().catch((e) => log(`announce: ${e}`));
    });
    c.on("data", (chunk) => {
      buffer += chunk;
      let i;
      while ((i = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, i);
        buffer = buffer.slice(i + 1);
        onLine(line);
      }
    });
    c.on("error", () => {}); // "close" follows; the daemon may just not be running.
    c.on("close", () => {
      if (conn === c) conn = null;
      buffer = "";
      if (stopped) return;
      clearTimeout(timer);
      timer = setTimeout(connect, retry);
      retry = Math.min(retry * 2, 30000);
    });
  }

  const disposables = [
    vscode.window.onDidOpenTerminal(announceSoon),
    vscode.window.onDidCloseTerminal((t) => {
      ids.delete(t);
      announceSoon();
    }),
  ];
  connect();

  return {
    connected: () => !!conn && !conn.destroyed && !conn.connecting,
    reconnect() {
      retry = opts.retryMs || 1000;
      if (conn) conn.destroy();
      else connect();
    },
    dispose() {
      stopped = true;
      clearTimeout(timer);
      clearTimeout(announceTimer);
      disposables.forEach((d) => d.dispose());
      if (conn) conn.destroy();
    },
  };
}

/** @param {any} context */
function activate(context) {
  const vscode = require("vscode");
  const output = vscode.window.createOutputChannel("aulora");
  const bridge = create(vscode, { log: (m) => output.appendLine(m) });
  context.subscriptions.push(output, bridge);
  context.subscriptions.push(
    vscode.commands.registerCommand("aulora.reconnect", () => bridge.reconnect()),
  );
}

function deactivate() {}

module.exports = { activate, deactivate, create, socketPath, keystrokes };
