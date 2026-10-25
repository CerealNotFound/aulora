//! Made-up sessions for `aulora demo`, driven through the real reducer.

use aulora_core::model::*;
use aulora_core::proto::{HookEnv, HookEnvelope};
use aulora_core::reduce::{apply_hook, push_context, trim_payload};
use serde_json::{Value, json};

use crate::git::{Group, Kind};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo).max(1)
    }
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/Users/you".into())
}

fn root(name: &str) -> String {
    format!("{}/Documents/projects/{name}", home())
}

pub struct Demo {
    pub snap: Snapshot,
    next_step: Ms,
    step: usize,
    rng: Lcg,
}

fn env_for(app: &str) -> HookEnv {
    let (program, bundle) = match app {
        "vscode" => ("vscode", "com.microsoft.VSCode"),
        "ghostty" => ("ghostty", "com.mitchellh.ghostty"),
        "cursor" => ("vscode", "com.todesktop.230313mzl4w4u92"),
        _ => ("iTerm.app", "com.googlecode.iterm2"),
    };
    HookEnv {
        term_program: Some(program.into()),
        bundle_id: Some(bundle.into()),
        pane_id: None,
        project_dir: None,
        tmux: false,
        claude_pid: None,
    }
}

impl Demo {
    fn hook(&mut self, id: &str, at: Ms, payload: Value) {
        let Some(s) = self.snap.sessions.iter_mut().find(|s| s.id == id) else { return };
        let mut p = payload;
        p["session_id"] = json!(id);
        if p.get("cwd").is_none() {
            p["cwd"] = json!(s.cwd.clone());
        }
        let app = match s.terminal.app_name().as_deref() {
            Some("VS Code") => "vscode",
            Some("Ghostty") => "ghostty",
            Some("Cursor") => "cursor",
            _ => "iterm",
        };
        // A session in one of aulora's panes reports from that pane, as
        // its real hooks do, so it stays linked to it.
        let env = match s.terminal.pane_id.clone() {
            Some(pane) => HookEnv {
                term_program: Some("aulora".into()),
                bundle_id: None,
                pane_id: Some(pane),
                project_dir: None,
                tmux: false,
                claude_pid: None,
            },
            None => env_for(app),
        };
        let env = HookEnvelope { v: 1, at, payload: trim_payload(p), env };
        let feed = apply_hook(s, &env);
        if let Some(proj) = self.snap.projects.iter_mut().find(|p| p.root == s.project_root) {
            proj.last_activity = proj.last_activity.max(at);
        }
        self.snap.feed.extend(feed);
        let len = self.snap.feed.len();
        if len > 5_000 {
            self.snap.feed.drain(0..len - 5_000);
        }
    }

    fn add_session(&mut self, id: &str, project: &str, app: &str, model: &str, started: Ms, title: &str) {
        let r = root(project);
        let mut s = Session::new(id, &r, &r, started);
        s.terminal = TerminalInfo {
            program: env_for(app).term_program,
            bundle_id: env_for(app).bundle_id,
            pane_id: None,
            tmux: false,
            editor: match app {
                "vscode" => Some("Visual Studio Code".into()),
                "cursor" => Some("Cursor".into()),
                _ => None,
            },
        };
        s.title = Some(title.into());
        s.pid = Some(40000 + (self.rng.next() % 9000) as u32);
        self.snap.sessions.push(s);
        self.hook(
            id,
            started,
            json!({"hook_event_name":"SessionStart","source":"startup","model":model,
            "permission_mode":"default","effort":{"level":"high"}}),
        );
    }

    /// A burst of past work so the sparklines and counters have history.
    fn history(&mut self, id: &str, from: Ms, to: Ms, files: &[&str], prompts: &[&str]) {
        let span = (to - from).max(1);
        let turns = prompts.len().max(1) as i64;
        for (ti, prompt) in prompts.iter().enumerate() {
            let t0 = from + span * ti as i64 / turns;
            let t1 = from + span * (ti as i64 + 1) / turns - 20_000;
            self.hook(id, t0, json!({"hook_event_name":"UserPromptSubmit","prompt":prompt}));
            let mut t = t0 + 3_000;
            while t < t1 {
                let tool = match self.rng.range(0, 6) {
                    0 | 1 => "Read",
                    2 => "Grep",
                    3 | 4 => "Edit",
                    _ => "Bash",
                };
                let file = files[self.rng.range(0, files.len() as u64) as usize];
                let path = format!("{}/{}", self.snap.sessions.iter().find(|s| s.id == id).unwrap().cwd, file);
                let n_new = self.rng.range(1, 30);
                let n_old = self.rng.range(0, 12);
                let cmd =
                    ["pnpm test", "cargo test -p core", "git status", "pnpm lint --fix"][self.rng.range(0, 4) as usize];
                let input = match tool {
                    "Bash" => json!({ "command": cmd }),
                    "Grep" => json!({"pattern":"TODO|FIXME","path": path}),
                    "Edit" => {
                        json!({"file_path": path, "old_string": "x\n".repeat(n_old as usize), "new_string": "y\n".repeat(n_new as usize)})
                    }
                    _ => json!({"file_path": path}),
                };
                let tid = format!("toolu_{}", self.rng.next());
                self.hook(
                    id,
                    t,
                    json!({"hook_event_name":"PreToolUse","tool_name":tool,"tool_use_id":tid,"tool_input":input}),
                );
                let dur = self.rng.range(40, 4000) as i64;
                self.hook(id, t + dur, json!({"hook_event_name":"PostToolUse","tool_name":tool,"tool_use_id":tid,"tool_input":input,"duration_ms":dur}));
                t += dur + self.rng.range(2_000, 25_000) as i64;
            }
            if ti + 1 < prompts.len() {
                self.hook(id, t1, json!({"hook_event_name":"Stop","last_assistant_message":"Done. Tests pass."}));
                let ctx = 30_000 + ti as u64 * 18_000 + self.rng.range(0, 8_000);
                if let Some(s) = self.snap.sessions.iter_mut().find(|s| s.id == id) {
                    s.context_tokens = Some(ctx);
                    push_context(s, ctx);
                }
            }
        }
    }

    fn set_tokens(&mut self, id: &str, ctx: u64, out: u64) {
        if let Some(s) = self.snap.sessions.iter_mut().find(|s| s.id == id) {
            s.context_tokens = Some(ctx);
            push_context(s, ctx);
            s.output_tokens = out;
        }
    }

    pub fn new() -> Demo {
        let now = now_ms();
        let min = 60_000;
        let mut d = Demo {
            snap: Snapshot {
                daemon: DaemonInfo {
                    version: aulora_core::VERSION.into(),
                    pid: 0,
                    started_at: now - 3 * 3_600_000 - 12 * min,
                    hooks_seen: 4_812,
                },
                projects: Vec::new(),
                sessions: Vec::new(),
                feed: Vec::new(),
                panes: Vec::new(),
            },
            next_step: now + 2_500,
            step: 0,
            rng: Lcg(42),
        };
        let git = |branch: &str, ahead, staged, modified, untracked, msg: &str, ago_min: i64| GitInfo {
            branch: Some(branch.into()),
            detached: false,
            ahead,
            behind: 0,
            staged,
            modified,
            untracked,
            conflicts: 0,
            last_commit: Some(msg.into()),
            last_commit_at: Some(now - ago_min * min),
        };
        for (name, g) in [
            ("adarshji", git("main", 1, 0, 3, 1, "fix: update GitHub username", 140)),
            ("multibrain", git("feat/credit-billing", 3, 2, 5, 0, "feat: per-request credit deduction", 35)),
            ("aulora", git("step-1-dashboard", 0, 0, 3, 2, "feat(daemon): hook ingest + sqlite store", 18)),
            ("cloudcast", git("main", 0, 0, 0, 0, "fix: shorter signed URL expiry", 300)),
            ("erwins-hire-ai", git("main", 0, 0, 1, 0, "perf: stream TTS chunks to avatar", 1500)),
        ] {
            d.snap.projects.push(Project {
                root: root(name),
                name: name.into(),
                git: Some(g),
                last_activity: now - 60 * min,
            });
        }

        // adarshji: blocked on a permission prompt.
        d.add_session(
            "7c8a516b-acfa-4ec0-96c2-e502e7641008",
            "adarshji",
            "vscode",
            "claude-opus-5-5",
            now - 52 * min,
            "Remove template SEO keywords",
        );
        d.history(
            "7c8a516b-acfa-4ec0-96c2-e502e7641008",
            now - 50 * min,
            now - 3 * min,
            &["config/site.ts", "app/(root)/page.tsx", "config/projects.ts", "public/projects/onetap/logo.png"],
            &[
                "remove the template SEO keywords and the SoftwareApplication schema",
                "add screenshots to every project card",
            ],
        );
        d.hook("7c8a516b-acfa-4ec0-96c2-e502e7641008", now - 2 * min, json!({"hook_event_name":"UserPromptSubmit","prompt":"clear the Next.js cache and rebuild so I can check the new OG image"}));
        d.hook("7c8a516b-acfa-4ec0-96c2-e502e7641008", now - 42_000, json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"rm -rf .next && pnpm build"}}));
        d.set_tokens("7c8a516b-acfa-4ec0-96c2-e502e7641008", 86_420, 21_300);

        // multibrain: two sessions, one deep in work with agents.
        d.add_session(
            "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31",
            "multibrain",
            "cursor",
            "claude-opus-5-5",
            now - 95 * min,
            "Credit billing → FastAPI",
        );
        d.history(
            "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31",
            now - 94 * min,
            now - min,
            &[
                "api/credits/route.py",
                "api/models/pricing.py",
                "web/app/billing/page.tsx",
                "api/tests/test_credits.py",
                "api/db/migrations/0012_credits.sql",
            ],
            &[
                "move credit deduction from the Next.js API routes into FastAPI",
                "add tests for concurrent deductions",
                "now wire the wallet page to the new endpoint",
            ],
        );
        d.hook(
            "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31",
            now - 50_000,
            json!({"hook_event_name":"SubagentStart","agent_type":"Explore","agent_id":"a1"}),
        );
        d.hook(
            "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31",
            now - 40_000,
            json!({"hook_event_name":"SubagentStart","agent_type":"code-reviewer","agent_id":"a2"}),
        );
        d.hook("3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31", now - 9_000, json!({"hook_event_name":"PreToolUse","tool_name":"Edit","tool_use_id":"live1",
            "tool_input":{"file_path": format!("{}/web/app/billing/page.tsx", root("multibrain")),"old_string":"a","new_string":"b\nc"}}));
        d.set_tokens("3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31", 142_310, 48_900);

        d.add_session(
            "9c2e4b7a-1d3f-4a5b-8c6d-2e1f0a9b8c7d",
            "multibrain",
            "cursor",
            "claude-sonnet-5-5",
            now - 40 * min,
            "Fix streaming when 6 models run at once",
        );
        d.history(
            "9c2e4b7a-1d3f-4a5b-8c6d-2e1f0a9b8c7d",
            now - 39 * min,
            now - 13 * min,
            &["web/lib/stream.ts", "web/components/chat/parallel-view.tsx"],
            &["responses get mixed up when 6 models stream in parallel, fix it"],
        );
        d.hook("9c2e4b7a-1d3f-4a5b-8c6d-2e1f0a9b8c7d", now - 12 * min, json!({"hook_event_name":"Stop",
            "last_assistant_message":"Fixed. Each model stream now gets its own AbortController and a stable request id, so chunks can't land in the wrong column. I also added a test that runs 6 fake streams with random delays. All 41 tests pass.\n\nWant me to also cap concurrent streams per user?"}));
        d.set_tokens("9c2e4b7a-1d3f-4a5b-8c6d-2e1f0a9b8c7d", 61_200, 9_800);

        // aulora: working on the next step, plus one waiting on a question.
        d.add_session(
            "e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f",
            "aulora",
            "ghostty",
            "claude-opus-5-5",
            now - 140 * min,
            "Daemon-owned terminals + screen state",
        );
        d.history(
            "e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f",
            now - 138 * min,
            now - min,
            &[
                "crates/aulora-daemon/src/pty.rs",
                "crates/aulora-daemon/src/screen.rs",
                "crates/aulora-core/src/proto.rs",
                "crates/aulora-tui/src/ui.rs",
            ],
            &[
                "spawn PTYs in the daemon with portable-pty",
                "keep a vt100 screen per pane so clients can redraw on attach",
                "stream pane output to subscribed clients",
            ],
        );
        d.hook("e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f", now - 14_000, json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"live2","tool_input":{"command":"cargo test -p aulora-daemon pty::"}}));
        d.set_tokens("e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f", 188_700, 71_400);

        d.add_session(
            "b5a4c3d2-e1f0-4a9b-8c7d-6e5f4a3b2c1d",
            "aulora",
            "vscode",
            "claude-opus-5-5",
            now - 25 * min,
            "README + demo GIF",
        );
        d.history(
            "b5a4c3d2-e1f0-4a9b-8c7d-6e5f4a3b2c1d",
            now - 24 * min,
            now - 2 * min,
            &["README.md", "docs/demo.tape"],
            &["write the README and record a demo with vhs"],
        );
        d.hook("b5a4c3d2-e1f0-4a9b-8c7d-6e5f4a3b2c1d", now - 95_000, json!({"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_use_id":"q1",
            "tool_input":{"questions":[{"question":"Which license should the README show: MIT, Apache-2.0, or both?","header":"License","multiSelect":false,
                "options":[{"label":"Both","description":"Dual MIT / Apache-2.0, as Rust crates usually are"},{"label":"MIT","description":"Shortest and most permissive"},{"label":"Apache-2.0","description":"Adds a patent grant"}]}]}}));
        d.set_tokens("b5a4c3d2-e1f0-4a9b-8c7d-6e5f4a3b2c1d", 38_900, 6_100);

        // cloudcast: idle, a fresh session.
        d.add_session(
            "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
            "cloudcast",
            "iterm",
            "claude-haiku-5-5",
            now - 6 * min,
            "session 1a2b3c4d",
        );
        if let Some(s) = d.snap.sessions.iter_mut().find(|s| s.id.starts_with("1a2b")) {
            s.title = None;
        }

        // erwins-hire-ai: ended earlier.
        d.add_session(
            "f0e1d2c3-b4a5-4968-8776-655443322110",
            "erwins-hire-ai",
            "vscode",
            "claude-opus-5-5",
            now - 80 * min,
            "Cut avatar response latency",
        );
        d.history(
            "f0e1d2c3-b4a5-4968-8776-655443322110",
            now - 79 * min,
            now - 31 * min,
            &["lib/tts/stream.ts", "app/interview/[id]/avatar.tsx"],
            &["the avatar takes 3s to start talking, stream TTS chunks instead"],
        );
        d.hook("f0e1d2c3-b4a5-4968-8776-655443322110", now - 30 * min, json!({"hook_event_name":"Stop","last_assistant_message":"First audio now plays after ~600ms instead of ~3s."}));
        d.hook(
            "f0e1d2c3-b4a5-4968-8776-655443322110",
            now - 21 * min,
            json!({"hook_event_name":"SessionEnd","reason":"prompt_input_exit"}),
        );
        d.set_tokens("f0e1d2c3-b4a5-4968-8776-655443322110", 74_000, 15_200);

        d.add_panes(now);
        d.write_transcripts(now);

        for (id, text) in [
            (
                "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31",
                "The FastAPI endpoint is live and the tests pass. Three places in the web app still deduct credits on the client; I'm moving them to the new `/credits/deduct` call.\n\nUpdating the wallet page next so the balance refreshes from the server after each request.",
            ),
            (
                "e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f",
                "Each pane now owns a vt100 parser that mirrors the PTY output, so a client that attaches late gets the current screen instead of a raw byte replay.\n\nRunning the pty tests to check resize handling.",
            ),
        ] {
            if let Some(s) = d.snap.sessions.iter_mut().find(|s| s.id == id) {
                s.live_text = Some(text.into());
            }
        }
        d.snap.feed.sort_by_key(|e| e.at);
        d
    }

    /// Real transcript files for a few sessions, so the Conversation tab
    /// shows the demo through the same code as a real session.
    fn write_transcripts(&mut self, now: Ms) {
        let dir = std::env::temp_dir().join("aulora-demo");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let iso = |ms: Ms| {
            chrono::DateTime::from_timestamp_millis(ms)
                .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                .unwrap_or_default()
        };
        let min = 60_000;
        let mut lines: Vec<(String, Vec<String>)> = Vec::new();
        let user = |at: Ms, text: &str| {
            json!({"type":"user","timestamp":iso(at),"message":{"role":"user","content":text}}).to_string()
        };
        let say = |at: Ms, id: &str, text: &str| {
            json!({"type":"assistant","timestamp":iso(at),"message":{"id":id,"content":[{"type":"text","text":text}]}})
                .to_string()
        };
        let tool = |at: Ms, id: &str, tid: &str, name: &str, input: Value| {
            json!({"type":"assistant","timestamp":iso(at),"message":{"id":id,"content":[{"type":"tool_use","id":tid,"name":name,"input":input}]}}).to_string()
        };
        let result = |tid: &str, out: &str, err: bool| {
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":tid,"content":out,"is_error":err}]}}).to_string()
        };
        let mb = root("multibrain");
        let ad = root("adarshji");

        lines.push(("3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31".into(), vec![
            json!({"type":"user","timestamp":iso(now - 94 * min),"cwd":mb,"message":{"role":"user","content":"move credit deduction from the Next.js API routes into FastAPI"}}).to_string(),
            say(now - 93 * min, "m1", "I'll find every place credits are deducted today, then move the logic behind one FastAPI endpoint."),
            tool(now - 93 * min, "m1", "t1", "Grep", json!({"pattern":"deductCredits|balance -", "path": format!("{mb}/web")})),
            result("t1", "Found 4 files\nweb/app/api/chat/route.ts\nweb/app/api/compare/route.ts\nweb/app/billing/page.tsx\nweb/lib/credits.ts", false),
            tool(now - 92 * min, "m2", "t2", "Read", json!({"file_path": format!("{mb}/web/lib/credits.ts")})),
            result("t2", &"x\n".repeat(88), false),
            tool(now - 90 * min, "m3", "t3", "Write", json!({"file_path": format!("{mb}/api/credits/route.py"), "content": "from fastapi import APIRouter, Depends\n\nrouter = APIRouter()\n\n@router.post('/credits/deduct')\nasync def deduct(req: DeductRequest, user=Depends(current_user)):\n    cost = price(req.model, req.tokens)\n    remaining = await wallet.debit(user.id, cost)\n    return {'remaining': remaining}\n"})),
            result("t3", "File created successfully", false),
            tool(now - 70 * min, "m4", "t4", "Bash", json!({"command":"pytest api/tests/test_credits.py -q"})),
            result("t4", "........................                                  [100%]\n24 passed in 1.84s", false),
            say(now - 69 * min, "m5", "The FastAPI endpoint is live and the tests pass. **Three places** in the web app still deduct credits on the client; I'll move them next."),
            user(now - 30 * min, "now wire the wallet page to the new endpoint"),
            tool(now - 29 * min, "m6", "t5", "Edit", json!({"file_path": format!("{mb}/web/app/billing/page.tsx"), "old_string":"    const remaining = balance - cost\n    setBalance(remaining)", "new_string":"    const { remaining } = await deductCredits(model, tokens)\n    setBalance(remaining) // the server is the source of truth"})),
            result("t5", "The file web/app/billing/page.tsx has been updated.", false),
            tool(now - 20 * min, "m7", "t6", "Bash", json!({"command":"pnpm lint --fix"})),
            result("t6", "web/app/billing/page.tsx\n  91:9  error  'cost' is assigned a value but never used  no-unused-vars\n\n✖ 1 problem (1 error, 0 warnings)", true),
            say(now - 19 * min, "m8", "One leftover variable. Removing it and updating the page so the balance refreshes from the server after each request."),
            tool(now - 9_000, "m9", "live1", "Edit", json!({"file_path": format!("{mb}/web/app/billing/page.tsx"), "old_string":"    const cost = estimateCost(model, tokens)\n", "new_string":""})),
        ]));
        lines.push(("7c8a516b-acfa-4ec0-96c2-e502e7641008".into(), vec![
            json!({"type":"user","timestamp":iso(now - 50 * min),"cwd":ad,"message":{"role":"user","content":"remove the template SEO keywords and the SoftwareApplication schema"}}).to_string(),
            tool(now - 49 * min, "a1", "s1", "Edit", json!({"file_path": format!("{ad}/config/site.ts"), "old_string":"    \"Next.js portfolio template\",\n    \"Free portfolio template\",\n    \"Next.js 13 template\",", "new_string":""})),
            result("s1", "updated", false),
            say(now - 48 * min, "a2", "Removed the template keywords from `config/site.ts` and the template schema from the home page."),
            user(now - 2 * min, "clear the Next.js cache and rebuild so I can check the new OG image"),
            say(now - 2 * min + 3_000, "a3", "Clearing `.next` and rebuilding:"),
            tool(now - 42_000, "a4", "s2", "Bash", json!({"command":"rm -rf .next && pnpm build"})),
        ]));
        for (id, rows) in lines {
            let path = dir.join(format!("{id}.jsonl"));
            if std::fs::write(&path, rows.join("\n") + "\n").is_ok()
                && let Some(s) = self.snap.sessions.iter_mut().find(|s| s.id == id)
            {
                s.transcript_path = Some(path.to_string_lossy().to_string());
            }
        }
    }

    fn add_panes(&mut self, now: Ms) {
        let pane =
            |id: &str, project: &str, name: &str, kind: PaneKind, args: &[&str], cols, rows, age_min: i64| Pane {
                id: id.into(),
                project_root: root(project),
                cwd: root(project),
                name: name.into(),
                kind,
                args: args.iter().map(|a| a.to_string()).collect(),
                pid: Some(51_000 + cols as u32),
                cols,
                rows,
                created_at: now - age_min * 60_000,
                last_output_at: now - 1_500,
                bytes_out: 0,
                title: None,
                bell_at: None,
                exit: None,
                command_exit: None,
                generation: 0,
            };
        self.snap.panes = vec![
            pane("a41c07", "aulora", "claude", PaneKind::Claude, &[], 104, 40, 140),
            pane("5d2e19", "multibrain", "claude", PaneKind::Claude, &[], 104, 40, 95),
            pane("c8f310", "multibrain", "dev server", PaneKind::Command, &["pnpm", "dev"], 104, 40, 96),
        ];
        for (session, pane_id) in
            [("e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f", "a41c07"), ("3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31", "5d2e19")]
        {
            if let Some(s) = self.snap.sessions.iter_mut().find(|s| s.id == session) {
                s.terminal = TerminalInfo {
                    program: Some("aulora".into()),
                    bundle_id: None,
                    pane_id: Some(pane_id.into()),
                    tmux: false,
                    editor: None,
                };
            }
        }
    }

    /// Advance the script. Returns true when something changed.
    pub fn tick(&mut self, now: Ms) -> bool {
        if now < self.next_step {
            return false;
        }
        self.next_step = now + 3_200;
        let mb = "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31";
        let au = "e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f";
        let mb2 = "9c2e4b7a-1d3f-4a5b-8c6d-2e1f0a9b8c7d";
        let mbr = root("multibrain");
        let aur = root("aulora");
        let steps: Vec<(&str, Value)> = vec![
            (
                mb,
                json!({"hook_event_name":"PostToolUse","tool_name":"Edit","tool_use_id":"live1","tool_input":{"file_path":format!("{mbr}/web/app/billing/page.tsx"),"old_string":"a","new_string":"b\nc"}}),
            ),
            (
                au,
                json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"live2","tool_input":{"command":"cargo test -p aulora-daemon pty::"}}),
            ),
            (
                mb,
                json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"live3","tool_input":{"command":"pytest api/tests/test_credits.py -q"}}),
            ),
            (
                au,
                json!({"hook_event_name":"PreToolUse","tool_name":"Edit","tool_use_id":"live4","tool_input":{"file_path":format!("{aur}/crates/aulora-daemon/src/screen.rs"),"old_string":"x","new_string":"y\nz\nw"}}),
            ),
            (
                mb,
                json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"live3","tool_input":{"command":"pytest api/tests/test_credits.py -q"}}),
            ),
            (
                mb2,
                json!({"hook_event_name":"UserPromptSubmit","prompt":"yes, cap it at 3 concurrent streams per user on the free plan"}),
            ),
            (
                au,
                json!({"hook_event_name":"PostToolUse","tool_name":"Edit","tool_use_id":"live4","tool_input":{"file_path":format!("{aur}/crates/aulora-daemon/src/screen.rs"),"old_string":"x","new_string":"y\nz\nw"}}),
            ),
            (
                mb2,
                json!({"hook_event_name":"PreToolUse","tool_name":"Grep","tool_use_id":"live5","tool_input":{"pattern":"MAX_CONCURRENT","path":format!("{mbr}/web/lib")}}),
            ),
            (
                mb,
                json!({"hook_event_name":"SubagentStop","agent_type":"Explore","agent_id":"a1","last_assistant_message":"Found 3 call sites that still deduct credits client-side."}),
            ),
            (
                au,
                json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"live2","tool_input":{"command":"cargo test -p aulora-daemon pty::"}}),
            ),
            (
                mb2,
                json!({"hook_event_name":"PostToolUse","tool_name":"Grep","tool_use_id":"live5","tool_input":{"pattern":"MAX_CONCURRENT"}}),
            ),
            (
                mb,
                json!({"hook_event_name":"PreToolUse","tool_name":"Edit","tool_use_id":"live1","tool_input":{"file_path":format!("{mbr}/web/app/billing/page.tsx"),"old_string":"a","new_string":"b\nc"}}),
            ),
            (
                mb2,
                json!({"hook_event_name":"Stop","last_assistant_message":"Done: free-plan users are capped at 3 concurrent streams; extra requests queue and show a ‘waiting for a free slot’ chip."}),
            ),
        ];
        let (id, payload) = steps[self.step % steps.len()].clone();
        self.step += 1;
        self.hook(id, now, payload);
        self.snap.daemon.hooks_seen += 1;
        true
    }
}

const R: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[38;2;110;231;150m";
const RED: &str = "\x1b[38;2;248;113;113m";
const ORANGE: &str = "\x1b[38;2;217;119;87m";
const BLUE: &str = "\x1b[38;2;125;166;255m";
const GREY: &str = "\x1b[38;2;136;145;168m";

fn lines(rows: &[String]) -> Vec<u8> {
    rows.join("\r\n").into_bytes()
}

/// Canned screens for the demo panes: (pane id, cols, rows, bytes).
pub fn screens() -> Vec<(String, u16, u16, Vec<u8>)> {
    let claude_multibrain = lines(&[
        format!("{ORANGE}✻{R} {BOLD}Credit billing → FastAPI{R}"),
        String::new(),
        format!("{GREY}>{R} now wire the wallet page to the new endpoint"),
        String::new(),
        "● The FastAPI endpoint is live and the tests pass. Three places in the web app".into(),
        "  still deduct credits on the client; I'm moving them to the new endpoint.".into(),
        String::new(),
        format!("● {BOLD}Read{R}(web/app/billing/page.tsx)"),
        format!("  {GREY}⎿  Read 214 lines{R}"),
        String::new(),
        format!("● {BOLD}Update{R}(web/app/billing/page.tsx)"),
        format!("  {GREY}⎿  Updated web/app/billing/page.tsx with 12 additions and 4 removals{R}"),
        format!("       {GREY}86{R}      const cost = estimateCost(model, tokens)"),
        format!("       {GREY}87{R} {RED}-    const remaining = balance - cost{R}"),
        format!("       {GREY}88{R} {RED}-    setBalance(remaining){R}"),
        format!("       {GREY}87{R} {GREEN}+    const {{ remaining }} = await deductCredits(model, tokens){R}"),
        format!("       {GREY}88{R} {GREEN}+    setBalance(remaining)   // server is the source of truth{R}"),
        String::new(),
        format!("● {BOLD}Bash{R}(pytest api/tests/test_credits.py -q)"),
        format!("  {GREY}⎿  ........................                                  [100%]{R}"),
        format!("     {GREY}24 passed in 1.84s{R}"),
        String::new(),
        format!("● {BOLD}Update{R}(web/app/billing/page.tsx)"),
        String::new(),
        format!("{ORANGE}✶ Wiring the wallet page…{R} {GREY}(1m 12s · ↓ 2.1k tokens · esc to interrupt){R}"),
        String::new(),
        format!(
            "{GREY}╭──────────────────────────────────────────────────────────────────────────────────────────────────╮{R}"
        ),
        format!(
            "{GREY}│{R} >                                                                                                {GREY}│{R}"
        ),
        format!(
            "{GREY}╰──────────────────────────────────────────────────────────────────────────────────────────────────╯{R}"
        ),
        format!("  {GREY}⏵⏵ accept edits on (shift+tab to cycle){R}"),
    ]);
    let claude_aulora = lines(&[
        format!("{ORANGE}✻{R} {BOLD}Daemon-owned terminals + screen state{R}"),
        String::new(),
        format!("{GREY}>{R} stream pane output to subscribed clients"),
        String::new(),
        "● Each pane now owns a vt100 parser that mirrors the PTY output, so a client".into(),
        "  that attaches late gets the current screen instead of a raw byte replay.".into(),
        String::new(),
        format!("● {BOLD}Bash{R}(cargo test -p aulora-daemon pty::)"),
        format!("  {GREY}⎿  running 4 tests{R}"),
        format!("     test pty::tests::claude_panes_fall_back_to_a_shell ... {GREEN}ok{R}"),
        format!("     test pty::tests::runs_a_command_and_reports_exit ... {GREEN}ok{R}"),
        format!("     test pty::tests::resize_updates_screen_and_pty ... {GREEN}ok{R}"),
        format!("     test pty::tests::input_reaches_the_program_and_attach_replays_the_screen ... {GREEN}ok{R}"),
        format!("     {GREY}test result: {R}{GREEN}ok{R}{GREY}. 4 passed; 0 failed; finished in 0.35s{R}"),
        String::new(),
        format!("{ORANGE}✶ Running the pty tests…{R} {GREY}(14s · esc to interrupt){R}"),
        String::new(),
        format!(
            "{GREY}╭──────────────────────────────────────────────────────────────────────────────────────────────────╮{R}"
        ),
        format!(
            "{GREY}│{R} >                                                                                                {GREY}│{R}"
        ),
        format!(
            "{GREY}╰──────────────────────────────────────────────────────────────────────────────────────────────────╯{R}"
        ),
    ]);
    let dev = lines(&[
        format!("{GREY}${R} pnpm dev"),
        String::new(),
        format!("{GREY}> multibrain-web@0.4.0 dev{R}"),
        format!("{GREY}> next dev --turbopack{R}"),
        String::new(),
        format!("   {BOLD}▲ Next.js 16.0.10{R} (Turbopack)"),
        format!("   - Local:        {BLUE}http://localhost:3000{R}"),
        format!("   - Network:      {BLUE}http://192.168.1.12:3000{R}"),
        String::new(),
        format!(" {GREEN}✓{R} Starting..."),
        format!(" {GREEN}✓{R} Ready in 812ms"),
        format!(" {GREY}○ Compiling /billing ...{R}"),
        format!(" {GREEN}✓{R} Compiled /billing in 1.2s"),
        format!(" GET /billing {GREEN}200{R} in 1361ms"),
        format!(" POST /api/credits/deduct {GREEN}200{R} in 48ms"),
        format!(" POST /api/credits/deduct {GREEN}200{R} in 41ms"),
        format!(" GET /api/wallet {GREEN}200{R} in 22ms"),
        format!(" {DIM}○ Compiling /billing ...{R}"),
        format!(" {GREEN}✓{R} Compiled /billing in 284ms"),
    ]);
    vec![
        ("5d2e19".into(), 104, 40, claude_multibrain),
        ("a41c07".into(), 104, 40, claude_aulora),
        ("c8f310".into(), 104, 40, dev),
    ]
}

// ---------------------------------------------------------------------------
// Changes for the demo's projects, so the Changes tab has something real-
// looking without reading the folders on this machine.

const MB_BILLING: &str = "3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31";
const MB_STREAM: &str = "9c2e4b7a-1d3f-4a5b-8c6d-2e1f0a9b8c7d";
const AD_SEO: &str = "7c8a516b-acfa-4ec0-96c2-e502e7641008";
const AU_STEP: &str = "e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f";

/// (project, group, kind, path, sessions, diff body or new file's content)
type DemoChange = (&'static str, Group, Kind, &'static str, &'static [&'static str], &'static str);

const DEMO_CHANGES: &[DemoChange] = &[
    ("multibrain", Group::Staged, Kind::Modified, "api/billing/credits.py", &[MB_BILLING], "@@ -18,9 +18,21 @@ from .models import Wallet
 
 
-def deduct(wallet: Wallet, cost: int) -> int:
-    wallet.balance -= cost
-    return wallet.balance
+async def deduct_credits(user_id: str, model: str, tokens: int) -> Deduction:
+    \"\"\"Charge a request on the server, so the client can't skip it.\"\"\"
+    cost = estimate_cost(model, tokens)
+    async with db.transaction():
+        wallet = await Wallet.for_update(user_id)
+        if wallet.balance < cost:
+            raise InsufficientCredits(needed=cost, balance=wallet.balance)
+        wallet.balance -= cost
+        await wallet.save()
+    return Deduction(remaining=wallet.balance, cost=cost)
+
+
+PRICES = {\"opus\": 15, \"sonnet\": 3, \"haiku\": 1}
"),
    ("multibrain", Group::Staged, Kind::Added, "api/tests/test_credits.py", &[MB_BILLING], "@@ -0,0 +1,12 @@
+import pytest
+
+from api.billing.credits import InsufficientCredits, deduct_credits
+
+
+async def test_deducts_on_the_server(wallet):
+    result = await deduct_credits(wallet.user_id, \"sonnet\", 1200)
+    assert result.remaining == 96
+
+async def test_refuses_when_short(empty_wallet):
+    with pytest.raises(InsufficientCredits):
+        await deduct_credits(empty_wallet.user_id, \"opus\", 9000)
"),
    ("multibrain", Group::Changes, Kind::Modified, "web/app/billing/page.tsx", &[MB_BILLING], "@@ -84,12 +84,14 @@ export default function BillingPage() {
   const { balance, setBalance } = useWallet();
   const [busy, setBusy] = useState(false);
 
   async function run(model: string, tokens: number) {
     setBusy(true);
-    const cost = estimateCost(model, tokens);
-    const remaining = balance - cost;
-    setBalance(remaining);
+    const { remaining } = await deductCredits(model, tokens);
+    setBalance(remaining); // the server is the source of truth
     setBusy(false);
   }
 
+  if (balance === undefined) return <BalanceSkeleton />;
+
   return (
     <main className=\"mx-auto max-w-3xl p-8\">
@@ -131,7 +133,7 @@ export default function BillingPage() {
       <PlanCard
         plan={plan}
-        onUpgrade={() => router.push(\"/pricing\")}
+        onUpgrade={() => router.push(\"/billing/upgrade\")}
         balance={balance}
       />
"),
    ("multibrain", Group::Changes, Kind::Modified, "web/lib/credits.ts", &[MB_BILLING], "@@ -1,8 +1,15 @@
-export function estimateCost(model: string, tokens: number): number {
-  return Math.ceil((tokens / 1000) * PRICES[model]);
+import { api } from \"./api\";
+
+export type Deduction = { remaining: number; cost: number };
+
+/** Charge a request through the API; never trust a client-side balance. */
+export async function deductCredits(model: string, tokens: number): Promise<Deduction> {
+  const res = await api.post(\"/billing/deduct\", { model, tokens });
+  if (res.status === 402) throw new InsufficientCreditsError(await res.json());
+  return res.json();
 }
"),
    ("multibrain", Group::Changes, Kind::Modified, "web/lib/stream.ts", &[MB_STREAM], "@@ -42,10 +42,14 @@ export async function streamAll(models: string[], prompt: string) {
-  const controller = new AbortController();
   return Promise.all(
     models.map(async (model, column) => {
-      const res = await fetch(`/api/chat/${model}`, { signal: controller.signal, body: prompt });
+      // One controller and request id per stream, so chunks can't cross columns.
+      const controller = new AbortController();
+      const requestId = crypto.randomUUID();
+      controllers.set(column, controller);
+      const res = await fetch(`/api/chat/${model}`, { signal: controller.signal, body: prompt, headers: { \"x-request-id\": requestId } });
       for await (const chunk of readChunks(res)) {
-        push(column, chunk);
+        if (chunk.requestId === requestId) push(column, chunk);
       }
     }),
   );
"),
    ("multibrain", Group::Changes, Kind::Modified, "web/components/chat/parallel-view.tsx", &[MB_STREAM], "@@ -27,6 +27,7 @@ export function ParallelView({ models }: Props) {
   const columns = useColumns(models);
+  useEffect(() => () => abortAll(), []);
   return (
     <div className=\"grid grid-cols-3 gap-3\">
"),
    ("multibrain", Group::Changes, Kind::Modified, "README.md", &[], "@@ -12,3 +12,4 @@ Run six models side by side.
 ## Billing
 
 Credits are charged per request.
+Balances live on the server; the client only shows them.
"),
    ("adarshji", Group::Changes, Kind::Modified, "config/site.ts", &[AD_SEO], "@@ -9,11 +9,6 @@ export const siteConfig = {
   description: \"Adarsh Pandey: full-stack and AI engineer.\",
   keywords: [
-    \"Next.js portfolio template\",
-    \"Free portfolio template\",
-    \"Next.js 13 template\",
     \"Adarsh Pandey\",
     \"AI engineer\",
-    \"Tailwind CSS\",
-    \"shadcn/ui\",
   ],
"),
    ("adarshji", Group::Changes, Kind::Modified, "app/page.tsx", &[AD_SEO], "@@ -4,12 +4,6 @@ import { Hero } from \"@/components/hero\";
 export default function Home() {
-  const schema = {
-    \"@context\": \"https://schema.org\",
-    \"@type\": \"SoftwareApplication\",
-    name: \"Portfolio template\",
-  };
   return (
     <>
-      <script type=\"application/ld+json\">{JSON.stringify(schema)}</script>
       <Hero />
"),
    ("adarshji", Group::Changes, Kind::Modified, "app/opengraph-image.tsx", &[AD_SEO], "@@ -20,3 +20,3 @@ export default async function Image() {
-      <div style={{ fontSize: 64 }}>Portfolio</div>
+      <div style={{ fontSize: 64, fontWeight: 700 }}>Adarsh Pandey</div>
"),
    ("adarshji", Group::Changes, Kind::Untracked, "public/og-background.png", &[], ""),
    ("aulora", Group::Changes, Kind::Modified, "crates/aulora-daemon/src/pty.rs", &[AU_STEP], "@@ -88,6 +88,12 @@ impl Proc {
         let reader = pair.master.try_clone_reader()?;
+        // Mirror the screen so a client that attaches late sees it at once.
+        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 5000)));
+        let (tx, _) = broadcast::channel(256);
+        spawn_reader(reader, parser.clone(), tx.clone());
         Ok(Proc { pair, child, parser, tx })
     }
"),
    ("aulora", Group::Changes, Kind::Modified, "crates/aulora-daemon/src/hub.rs", &[AU_STEP], "@@ -140,7 +140,9 @@ impl Hub {
     pub fn attach(&mut self, pane: &str) -> Option<Attach> {
-        let p = self.panes.get(pane)?;
-        Some(Attach { screen: p.screen(), rx: p.subscribe() })
+        let p = self.panes.get(pane)?;
+        // Snapshot and subscribe under one lock, so no output falls between.
+        let (screen, rx) = p.snapshot_and_subscribe();
+        Some(Attach { screen, rx })
     }
"),
    ("aulora", Group::Changes, Kind::Modified, "README.md", &["b5a4c3d2-e1f0-4a9b-8c7d-6e5f4a3b2c1d"], "@@ -1,4 +1,6 @@
 # aulora
 
 A dashboard for every Claude Code session on your machine.
+
+![aulora](docs/screenshots/overview.png)
"),
    ("aulora", Group::Changes, Kind::Untracked, "crates/aulora-daemon/src/screen.rs", &[AU_STEP], "//! A pane's screen, kept current from its output.

use std::sync::{Arc, Mutex};

pub struct Screen {
    parser: Arc<Mutex<vt100::Parser>>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Screen {
        Screen { parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 5000))) }
    }

    pub fn feed(&self, bytes: &[u8]) {
        self.parser.lock().expect(\"screen lock\").process(bytes);
    }
}
"),
    ("aulora", Group::Changes, Kind::Untracked, "docs/screenshots/demo.gif", &[], ""),
    ("erwins-hire-ai", Group::Changes, Kind::Modified, "avatar/tts.py", &["f0e1d2c3-b4a5-4968-8776-655443322110"], "@@ -30,4 +30,6 @@ async def speak(text: str):
-    audio = await synthesize(text)
-    await avatar.play(audio)
+    # Start playing the first sentence while the rest is synthesized.
+    async for chunk in synthesize_stream(text):
+        await avatar.play(chunk)
"),
];

fn demo_diff(path: &str, kind: Kind, body: &str) -> crate::git::FileDiff {
    if kind == Kind::Untracked {
        if body.is_empty() {
            return crate::git::FileDiff { path: path.into(), binary: true, ..Default::default() };
        }
        return crate::git::added_file(path, body);
    }
    let text = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{body}");
    crate::git::parse_diff(&text).pop().unwrap_or_default()
}

pub fn changes(root_path: &str) -> Vec<crate::changes::FileChange> {
    let project = root_path.rsplit('/').next().unwrap_or("");
    DEMO_CHANGES
        .iter()
        .filter(|c| c.0 == project)
        .map(|(_, group, kind, path, sessions, body)| {
            let d = demo_diff(path, *kind, body);
            let (added, removed) = d.counts();
            crate::changes::FileChange {
                entry: crate::git::Entry { group: *group, kind: *kind, path: path.to_string(), orig: None },
                added,
                removed,
                binary: d.binary,
                sessions: sessions.iter().map(|s| s.to_string()).collect(),
                sig: 1,
            }
        })
        .collect()
}

pub fn diff(f: &crate::changes::FileChange) -> crate::highlight::Styled {
    let body = DEMO_CHANGES.iter().find(|c| c.3 == f.entry.path && c.1 == f.entry.group).map(|c| c.5).unwrap_or("");
    let d = demo_diff(&f.entry.path, f.entry.kind, body);
    let first = f.sessions.first().cloned();
    crate::highlight::style(&d, |_| first.clone())
}

// ---------------------------------------------------------------------------
// Files for the demo's projects: the changed files above plus a few more,
// so the Files tab and Ctrl-P have something to show.

const DEMO_FILES: &[(&str, &str)] = &[
    ("multibrain", "package.json"),
    ("multibrain", "api/main.py"),
    ("multibrain", "api/billing/__init__.py"),
    ("multibrain", "api/billing/models.py"),
    ("multibrain", "web/app/layout.tsx"),
    ("multibrain", "web/app/page.tsx"),
    ("multibrain", "web/lib/api.ts"),
    ("multibrain", "web/components/chat/column.tsx"),
    ("adarshji", "package.json"),
    ("adarshji", "components/hero.tsx"),
    ("aulora", "Cargo.toml"),
    ("aulora", "crates/aulora-daemon/src/lib.rs"),
];

fn demo_paths(root_path: &str) -> Vec<String> {
    let project = root_path.rsplit('/').next().unwrap_or("");
    let mut v: Vec<String> = DEMO_CHANGES
        .iter()
        .filter(|c| c.0 == project && c.2 != Kind::Deleted)
        .map(|c| c.3.to_string())
        .chain(DEMO_FILES.iter().filter(|f| f.0 == project).map(|f| f.1.to_string()))
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn index(root_path: &str) -> Vec<String> {
    demo_paths(root_path)
}

pub fn dirs(root_path: &str) -> Vec<(String, Vec<crate::files::DirEntry>)> {
    use std::collections::BTreeMap;
    let mut dirs: BTreeMap<String, Vec<crate::files::DirEntry>> = BTreeMap::new();
    dirs.entry(String::new()).or_default();
    for p in demo_paths(root_path) {
        let parts: Vec<&str> = p.split('/').collect();
        let mut dir = String::new();
        for (i, part) in parts.iter().enumerate() {
            let is_dir = i + 1 < parts.len();
            let list = dirs.entry(dir.clone()).or_default();
            if !list.iter().any(|e| e.name == *part) {
                list.push(crate::files::DirEntry { name: part.to_string(), is_dir, ignored: part == &"target" });
            }
            dir = if dir.is_empty() { part.to_string() } else { format!("{dir}/{part}") };
            if is_dir {
                dirs.entry(dir.clone()).or_default();
            }
        }
    }
    dirs.into_iter()
        .map(|(d, mut v)| {
            v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            (d, v)
        })
        .collect()
}

/// A demo file's text: the new side of its demo diff, marked, or a stub.
pub fn file_text(root_path: &str, rel: &str) -> crate::files::Text {
    let project = root_path.rsplit('/').next().unwrap_or("");
    let change = DEMO_CHANGES.iter().find(|c| c.0 == project && c.3 == rel);
    let (content, marks) = match change {
        Some((_, _, kind, path, _, body)) if !body.is_empty() => {
            let d = demo_diff(path, *kind, body);
            let mut lines = Vec::new();
            let mut marks = std::collections::HashMap::new();
            for l in d.hunks.iter().flat_map(|h| &h.lines) {
                if l.kind == crate::git::LineKind::Removed {
                    continue;
                }
                lines.push(l.text.clone());
                if l.kind == crate::git::LineKind::Added {
                    marks.insert(lines.len() as u32, crate::files::Mark::Added);
                }
            }
            (lines.join("\n"), marks)
        }
        _ => (format!("// {rel}\n// (demo file)\n"), Default::default()),
    };
    let lines: Vec<crate::git::DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, l)| crate::git::DiffLine {
            kind: crate::git::LineKind::Context,
            old: None,
            new: Some(i as u32 + 1),
            text: l.into(),
        })
        .collect();
    let n = lines.len();
    let diff = crate::git::FileDiff {
        path: rel.into(),
        hunks: vec![crate::git::Hunk { old_start: 0, new_start: 1, context: String::new(), lines }],
        binary: false,
    };
    crate::files::Text {
        styled: Some(std::sync::Arc::new(crate::highlight::style(&diff, |_| None))),
        binary: false,
        lines: n,
        marks,
        modified: None,
        truncated: false,
        deleted: false,
    }
}

pub fn search(root_path: &str, query: &str) -> Vec<crate::files::Match> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for p in demo_paths(root_path) {
        let t = file_text(root_path, &p);
        let Some(s) = t.styled else { continue };
        for l in s.hunks.iter().flat_map(|h| &h.lines) {
            let text: String = l.segs.iter().map(|g| g.text.as_str()).collect();
            if text.to_lowercase().contains(&q) {
                out.push(crate::files::Match { path: p.clone(), line: l.new.unwrap_or(0), text });
            }
        }
    }
    out
}

/// Layouts for the demo: Claude with Changes in multibrain, Claude with
/// Files in aulora.
pub fn layouts() -> crate::layout::Layouts {
    use crate::layout::{Layouts, Space, View};
    let mut l = Layouts::default();
    l.spaces.insert(
        root("multibrain"),
        Space {
            main: View::Pane { id: "5d2e19".into() },
            panel: Some(View::Changes { filter: Some(MB_BILLING.into()) }),
            ratio: 0.56,
            ..Space::default()
        },
    );
    l.spaces.insert(
        root("aulora"),
        Space { main: View::Pane { id: "a41c07".into() }, panel: Some(View::Files), ratio: 0.55, ..Space::default() },
    );
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_in_panes_stay_in_them_as_the_script_runs() {
        let mut d = Demo::new();
        for i in 1..40 {
            d.tick(d.next_step + i);
        }
        let pane = |id: &str| d.snap.sessions.iter().find(|s| s.id == id).and_then(|s| s.terminal.pane_id.clone());
        assert_eq!(pane("3f1d9a20-5b6c-4e1f-9d2a-8b7c6e5f4a31").as_deref(), Some("5d2e19"));
        assert_eq!(pane("e8d9964f-fda5-46e8-a6fd-94ace0fd4a1f").as_deref(), Some("a41c07"));
    }
}
