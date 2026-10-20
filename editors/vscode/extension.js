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
      if (msg.enter === false) {
        // Keys that answer a dialog, as they are.
        t.sendText(msg.text, false);
        send({ type: "typed", request_id: msg.request_id, error: null });
        return;
      }
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
    if (msg.type === "close") {
      const t = terminalById(msg.terminal_id);
      if (t) t.dispose();
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

/** One request on its own connection: send `msg`, resolve with the first reply. */
function request(socket, msg) {
  return new Promise((resolve, reject) => {
    const c = net.createConnection(socket);
    let buf = "";
    c.setEncoding("utf8");
    c.on("connect", () => c.write(JSON.stringify(msg) + "\n"));
    c.on("data", (d) => {
      buf += d;
      const i = buf.indexOf("\n");
      if (i >= 0) {
        c.destroy();
        try {
          resolve(JSON.parse(buf.slice(0, i)));
        } catch (e) {
          reject(e);
        }
      }
    });
    c.on("error", reject);
  });
}

/**
 * A daemon pane as a VS Code terminal: its screen and output come in,
 * keystrokes and resizes go out. Closing the terminal leaves the pane
 * running in the daemon.
 */
function paneTerminal(vscode, socket, pane) {
  const write = new vscode.EventEmitter();
  const closed = new vscode.EventEmitter();
  let conn = null;
  let buffer = "";
  const send = (msg) => conn && !conn.destroyed && conn.write(JSON.stringify(msg) + "\n");
  const pty = {
    onDidWrite: write.event,
    onDidClose: closed.event,
    open(dims) {
      conn = net.createConnection(socket);
      conn.setEncoding("utf8");
      conn.on("connect", () => {
        send({ type: "attach", pane_id: pane.id });
        if (dims) send({ type: "resize", pane_id: pane.id, cols: dims.columns, rows: dims.rows });
      });
      conn.on("data", (chunk) => {
        buffer += chunk;
        let i;
        while ((i = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, i);
          buffer = buffer.slice(i + 1);
          let msg;
          try {
            msg = JSON.parse(line);
          } catch {
            continue;
          }
          if ((msg.type === "pane_screen" || msg.type === "pane_output") && msg.pane_id === pane.id) {
            write.fire(Buffer.from(msg.data, "base64").toString("utf8"));
          } else if (msg.type === "pane_exited" && msg.pane_id === pane.id) {
            write.fire(`\r\n[exited with code ${msg.code}; restart it from aulora]\r\n`);
          }
        }
      });
      conn.on("error", () => {});
      conn.on("close", () => write.fire("\r\n[aulora daemon went away]\r\n"));
    },
    close() {
      send({ type: "detach", pane_id: pane.id });
      if (conn) conn.end();
    },
    handleInput(data) {
      send({ type: "input", pane_id: pane.id, data: Buffer.from(data, "utf8").toString("base64") });
    },
    setDimensions(dims) {
      send({ type: "resize", pane_id: pane.id, cols: dims.columns, rows: dims.rows });
    },
  };
  return vscode.window.createTerminal({ name: `aulora · ${pane.name}`, pty });
}

/** The daemon's panes, from a snapshot. */
async function panes(socket) {
  const reply = await request(socket, { type: "snapshot" });
  return (reply && reply.panes) || [];
}

/** @param {any} context */
function activate(context) {
  const vscode = require("vscode");
  const output = vscode.window.createOutputChannel("aulora");
  const bridge = create(vscode, { log: (m) => output.appendLine(m) });
  context.subscriptions.push(output, bridge);
  const socket = socketPath();
  const open = new Set();
  const show = (p) => {
    if (open.has(p.id)) return;
    open.add(p.id);
    const t = paneTerminal(vscode, socket, p);
    t.show();
    const sub = vscode.window.onDidCloseTerminal((closed) => {
      if (closed === t) {
        open.delete(p.id);
        sub.dispose();
      }
    });
  };
  context.subscriptions.push(
    vscode.commands.registerCommand("aulora.reconnect", () => bridge.reconnect()),
    vscode.commands.registerCommand("aulora.openPane", async () => {
      let list;
      try {
        list = await panes(socket);
      } catch {
        vscode.window.showWarningMessage("aulora isn't running (start it with `aulora`).");
        return;
      }
      if (!list.length) {
        vscode.window.showInformationMessage("aulora has no panes yet.");
        return;
      }
      const pick = await vscode.window.showQuickPick(
        list.map((p) => ({ label: p.name, description: p.project_root, detail: p.exit ? "exited" : "running", pane: p })),
        { placeHolder: "Open an aulora pane here" },
      );
      if (pick) show(pick.pane);
    }),
    vscode.commands.registerCommand("aulora.openProjectPanes", async () => {
      const folders = (vscode.workspace.workspaceFolders || []).map((f) => f.uri.fsPath);
      let list;
      try {
        list = await panes(socket);
      } catch {
        vscode.window.showWarningMessage("aulora isn't running (start it with `aulora`).");
        return;
      }
      const mine = list.filter((p) => folders.some((f) => p.project_root === f || p.cwd.startsWith(f + "/") || p.cwd === f));
      if (!mine.length) vscode.window.showInformationMessage("aulora has no panes for this window's folders.");
      mine.forEach(show);
    }),
  );
}

function deactivate() {}

module.exports = { activate, deactivate, create, socketPath, keystrokes, paneTerminal, request };
