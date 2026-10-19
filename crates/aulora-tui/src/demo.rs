//! Made-up sessions for `aulora demo`, driven through the real reducer.

use aulora_core::model::*;
use aulora_core::proto::{HookEnv, HookEnvelope};
use aulora_core::reduce::{apply_hook, push_context, trim_payload};
use serde_json::{Value, json};

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
        let env = HookEnvelope { v: 1, at, payload: trim_payload(p), env: env_for(app) };
        let feed = apply_hook(s, &env);
        if let Some(proj) = self.snap.projects.iter_mut().find(|p| p.root == s.project_root) {
            proj.last_activity = proj.last_activity.max(at);
        }
        self.snap.feed.extend(feed);
        let len = self.snap.feed.len();
        if len > 300 {
            self.snap.feed.drain(0..len - 300);
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
            ("aulora", git("step-1-dashboard", 0, 0, 7, 4, "feat(daemon): hook ingest + sqlite store", 18)),
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
            "ghostty",
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
            "tool_input":{"questions":[{"question":"Which license should the README show: MIT, Apache-2.0, or both?","header":"License"}]}}));
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
