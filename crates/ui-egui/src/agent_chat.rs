//! Opt-in native bridge to an existing Herdr agent. No model or agent is started here.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;

pub const MESSAGE_LIMIT: usize = 8 * 1024;
const REPLY_LIMIT: usize = 32 * 1024;
const ENTRY_LIMIT: usize = 64;
const FILE_LIMIT: usize = 3 * 1024 * 1024;
const CONFIG: &str = "Agent Chat is disabled. Relaunch with FILMCRAFT_AGENT_TARGET (existing Herdr target), FILMCRAFT_AGENT_CHAT_DIR (absolute persistent directory), and FILMCRAFT_AGENT_CONTROL_ADDR (live loopback host:port). This grants the agent control of this project; no agent is started.";

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    request_id: String,
    role: String,
    text: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Transcript {
    project_key: String,
    project_id: String,
    thread_id: String,
    entries: Vec<Entry>,
}

pub struct Chat {
    pub input: String,
    pub error: Option<String>,
    status: String,
    key: String,
    transcript: Option<Transcript>,
    request_id: Option<String>,
    reply_path: Option<String>,
    unsaved_id: String,
    #[cfg(not(target_arch = "wasm32"))]
    config: Option<native::Config>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<std::sync::mpsc::Receiver<Result<Transcript, String>>>,
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            input: String::new(),
            error: None,
            status: "disabled".into(),
            key: String::new(),
            transcript: None,
            request_id: None,
            reply_path: None,
            unsaved_id: unique_id(),
            #[cfg(not(target_arch = "wasm32"))]
            config: None,
            #[cfg(not(target_arch = "wasm32"))]
            worker: None,
        }
    }
}

fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let time = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let process = std::process::id();
    #[cfg(target_arch = "wasm32")]
    let process = 0;
    format!("{time:x}-{process:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

impl Chat {
    fn busy(&self) -> bool {
        matches!(self.status.as_str(), "loading" | "waiting")
    }

    pub fn inspect(&self) -> Value {
        let mut value = json!({
            "status": self.status, "error": self.error, "configuration": CONFIG,
            "projectId": self.transcript.as_ref().map(|t| &t.project_id),
            "threadId": self.transcript.as_ref().map(|t| &t.thread_id),
            "requestId": self.request_id, "replyPath": self.reply_path,
            "transcript": self.transcript.as_ref().map(|t| &t.entries),
            "input": self.input, "messageLimitBytes": MESSAGE_LIMIT,
            "replyLimitBytes": REPLY_LIMIT, "timeoutSeconds": 300,
        });
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(config) = &self.config {
            value["target"] = json!(config.target);
            value["controlAddress"] = json!(config.address);
            value["chatDirectory"] = json!(config.dir);
        }
        value
    }

    pub fn poll(&mut self, session: &filmcraft_engine::Session, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(rx) = &self.worker {
                match rx.try_recv() {
                    Ok(result) => {
                        self.worker = None;
                        match result {
                            Ok(t) => {
                                self.error = t.entries.last().filter(|e| e.role == "error").map(|e| e.text.clone());
                                self.status = if self.error.is_some() { "error" } else { "ready" }.into();
                                self.transcript = Some(t);
                            }
                            Err(e) => {
                                self.error = Some(e);
                                self.status = "error".into();
                            }
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.worker = None;
                        self.error = Some("Agent Chat worker disconnected".into());
                        self.status = "error".into();
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
            if self.busy() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            let key = session.path.clone().unwrap_or_else(|| format!("unsaved:{}", self.unsaved_id));
            if key != self.key {
                self.key = key.clone();
                self.transcript = None;
                self.error = None;
                self.request_id = None;
                self.reply_path = None;
                match native::Config::load() {
                    Ok(Some(config)) => {
                        let dir = config.dir.clone();
                        self.config = Some(config);
                        self.status = "loading".into();
                        match native::spawn(ctx.clone(), move || native::load(&dir, &key)) {
                            Ok(rx) => self.worker = Some(rx),
                            Err(e) => {
                                self.error = Some(e);
                                self.status = "error".into();
                            }
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    Ok(None) => {
                        self.config = None;
                        self.status = "disabled".into();
                    }
                    Err(e) => {
                        self.config = None;
                        self.error = Some(e);
                        self.status = "disabled".into();
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (session, ctx);
    }
}

/// Shared entry point for the native button and control-channel send.
pub fn send(app: &mut FilmcraftApp, ctx: &egui::Context, text: &str) -> Result<Value, String> {
    let result = send_inner(app, ctx, text);
    if let Err(e) = &result {
        app.agent_chat.error = Some(e.clone());
    }
    result
}

fn send_inner(app: &mut FilmcraftApp, ctx: &egui::Context, text: &str) -> Result<Value, String> {
    if text.trim().is_empty() || text.len() > MESSAGE_LIMIT {
        return Err(format!("Message must be nonempty and at most {MESSAGE_LIMIT} UTF-8 bytes"));
    }
    app.agent_chat.poll(&app.session, ctx);
    if app.agent_chat.busy() {
        return Err("Agent Chat is already loading or waiting for a reply".into());
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err("Agent Chat is native-only".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let config = app.agent_chat.config.clone().ok_or(CONFIG)?;
        let mut transcript = app.agent_chat.transcript.clone().ok_or("Transcript unavailable; reopen the project to retry loading")?;
        let request_id = unique_id();
        let reply_path = config.dir.join(format!("{request_id}.reply.json"));
        let summary = json!({
            "projectPath": app.session.path,
            "projectName": app.session.project.name.chars().take(512).collect::<String>(),
            "activeSequence": app.session.state.active_sequence,
            "sequenceName": app.session.state.active_sequence.and_then(|id| app.session.project.items.get(&id)).map(|item| item.name.chars().take(512).collect::<String>()),
            "selection": app.session.state.selection.iter().take(64).collect::<Vec<_>>(),
            "projectSelection": app.session.state.project_selection.iter().take(64).collect::<Vec<_>>(),
            "selectionTruncated": app.session.state.selection.len() > 64 || app.session.state.project_selection.len() > 64,
            "playhead": app.session.playhead(),
        });
        let request = json!({
            "requestId": request_id, "projectId": transcript.project_id, "threadId": transcript.thread_id,
            "message": text, "context": summary, "controlAddress": config.address,
            "responsePath": reply_path, "responseSchema": {"reply": "actual agent response"},
        });
        let request_bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if request_bytes.len() > 24 * 1024 {
            return Err("Request context exceeds 24 KiB".into());
        }
        transcript.entries.push(Entry { request_id: request_id.clone(), role: "user".into(), text: text.into() });
        native::trim(&mut transcript);
        let visible = transcript.clone();
        let id = request_id.clone();
        let reply_display = reply_path.to_string_lossy().into_owned();
        let rx = native::spawn(ctx.clone(), move || native::request(config, transcript, &id, request_bytes, &reply_path))?;
        let chat = &mut app.agent_chat;
        chat.worker = Some(rx);
        chat.transcript = Some(visible);
        chat.request_id = Some(request_id.clone());
        chat.reply_path = Some(reply_display.clone());
        chat.error = None;
        chat.status = "waiting".into();
        chat.input.clear();
        app.show_panel(crate::dock::PanelKind::AgentChat);
        ctx.request_repaint();
        Ok(json!({"requestId": request_id, "replyPath": reply_display, "status": "waiting"}))
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
    let ui = &mut child;
    ui.label(egui::RichText::new("Local agent • live project access").color(app.tokens.text_dim));
    if app.agent_chat.status == "disabled" {
        ui.label(CONFIG);
    }
    if let Some(e) = &app.agent_chat.error {
        ui.label(egui::RichText::new(e).color(app.tokens.danger));
    }
    if app.agent_chat.busy() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("{}…", app.agent_chat.status));
        });
    }
    let height = (ui.available_height() - 115.0).max(30.0);
    let scroll = egui::ScrollArea::vertical().id_salt("agent-chat-transcript").max_height(height).stick_to_bottom(true).show(ui, |ui| {
        if let Some(t) = &app.agent_chat.transcript {
            for entry in &t.entries {
                ui.label(egui::RichText::new(&entry.role).strong());
                ui.label(&entry.text);
                ui.separator();
            }
        }
    });
    app.auto.add("agent.transcript", scroll.inner_rect, "Agent Chat transcript");
    let input = ui.add(egui::TextEdit::multiline(&mut app.agent_chat.input).desired_rows(3).desired_width(f32::INFINITY).char_limit(MESSAGE_LIMIT));
    app.auto.add("agent.input", input.rect, "Message to existing agent");
    let enabled = !app.agent_chat.busy() && app.agent_chat.status != "disabled" && !app.agent_chat.input.trim().is_empty();
    let button = ui.add_enabled(enabled, egui::Button::new("Send"));
    app.auto.add("agent.send", button.rect, "Send agent message");
    if button.clicked() {
        let text = app.agent_chat.input.clone();
        let _ = send(app, ui.ctx(), &text); // send itself displays failures in the panel.
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    pub struct Config {
        pub target: String,
        pub dir: PathBuf,
        pub address: String,
    }
    impl Config {
        pub fn load() -> Result<Option<Self>, String> {
            let Ok(target) = std::env::var("FILMCRAFT_AGENT_TARGET") else {
                return Ok(None);
            };
            if target.is_empty() || target.len() > 128 || target.starts_with('-') || target.chars().any(char::is_whitespace) {
                return Err("Invalid FILMCRAFT_AGENT_TARGET".into());
            }
            let dir = PathBuf::from(std::env::var("FILMCRAFT_AGENT_CHAT_DIR").map_err(|_| CONFIG)?);
            if !dir.is_absolute() || dir.as_os_str().len() > 4096 {
                return Err("FILMCRAFT_AGENT_CHAT_DIR must be an absolute path of at most 4096 bytes".into());
            }
            let address = std::env::var("FILMCRAFT_AGENT_CONTROL_ADDR").map_err(|_| CONFIG)?;
            let parsed: std::net::SocketAddr = address.parse().map_err(|_| "Control address must be a numeric loopback IP:port")?;
            if !parsed.ip().is_loopback() || parsed.port() == 0 {
                return Err("Agent control address must be a loopback address with a nonzero port".into());
            }
            Ok(Some(Self { target, dir, address }))
        }
    }

    pub fn spawn(
        ctx: egui::Context,
        work: impl FnOnce() -> Result<Transcript, String> + Send + 'static,
    ) -> Result<std::sync::mpsc::Receiver<Result<Transcript, String>>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("filmcraft-agent-chat".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| Err("Agent Chat worker failed unexpectedly".into()));
                let _ = tx.send(result);
                ctx.request_repaint();
            })
            .map_err(|e| format!("Start Agent Chat worker: {e}"))?;
        Ok(rx)
    }

    fn transcript_path(dir: &Path, key: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hash);
        dir.join(format!("project-{:016x}.transcript.json", hash.finish()))
    }

    fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
        if !std::fs::symlink_metadata(path).map_err(|e| e.to_string())?.is_file() {
            return Err("Chat input must be a regular file, not a symlink or pipe".into());
        }
        let f = std::fs::File::open(path).map_err(|e| format!("Read {}: {e}", path.display()))?;
        if !f.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Chat input is not a regular file".into());
        }
        let mut data = Vec::new();
        f.take(max.saturating_add(1) as u64).read_to_end(&mut data).map_err(|e| e.to_string())?;
        if data.len() > max {
            return Err(format!("Chat file exceeds {max} bytes"));
        }
        Ok(data)
    }

    pub fn trim(t: &mut Transcript) {
        let excess = t.entries.len().saturating_sub(ENTRY_LIMIT);
        t.entries.drain(..excess);
        // JSON escaping can make bounded text grow sixfold. Keep the persisted tail bounded too.
        for _ in 0..ENTRY_LIMIT {
            if serde_json::to_vec(t).is_ok_and(|bytes| bytes.len() <= FILE_LIMIT) || t.entries.is_empty() {
                break;
            }
            t.entries.remove(0);
        }
    }

    fn save(dir: &Path, t: &Transcript) -> Result<(), String> {
        let bytes = serde_json::to_vec(t).map_err(|e| e.to_string())?;
        if bytes.len() > FILE_LIMIT {
            return Err("Transcript exceeds persistence limit".into());
        }
        let path = transcript_path(dir, &t.project_key);
        let tmp = dir.join(format!("{}.transcript.tmp", unique_id()));
        std::fs::write(&tmp, bytes).map_err(|e| format!("Persist transcript: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("Replace transcript {}: {e}", path.display()))
    }

    pub fn load(dir: &Path, key: &str) -> Result<Transcript, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create chat directory: {e}"))?;
        let path = transcript_path(dir, key);
        let mut t: Transcript = match std::fs::metadata(&path) {
            Ok(_) => serde_json::from_slice(&read_bounded(&path, FILE_LIMIT)?).map_err(|e| format!("Invalid transcript: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Transcript { project_key: key.into(), project_id: unique_id(), thread_id: unique_id(), entries: Vec::new() }
            }
            Err(e) => return Err(format!("Inspect transcript: {e}")),
        };
        if t.project_key != key
            || t.project_id.len() > 128
            || t.thread_id.len() > 128
            || t.entries.len() > ENTRY_LIMIT
            || t.entries.iter().any(|e| e.text.len() > REPLY_LIMIT || e.role.len() > 32 || e.request_id.len() > 128)
        {
            return Err("Transcript has invalid or oversized fields".into());
        }
        // A request interrupted by closing the app remains visible, never silently resent.
        if t.entries.last().is_some_and(|e| e.role == "user") {
            t.entries.push(Entry {
                request_id: String::new(),
                role: "error".into(),
                text: "Previous request was interrupted. Check its reply file before sending again; actions may already have run.".into(),
            });
            trim(&mut t);
        }
        save(dir, &t)?;
        Ok(t)
    }

    pub fn request(config: Config, mut t: Transcript, id: &str, bytes: Vec<u8>, reply: &Path) -> Result<Transcript, String> {
        save(&config.dir, &t)?;
        let result = dispatch(&config, &t, id, bytes, reply);
        let (role, text) = match result {
            Ok(text) => ("agent", text),
            Err(e) => ("error", e),
        };
        t.entries.push(Entry { request_id: id.into(), role: role.into(), text });
        trim(&mut t);
        save(&config.dir, &t)?;
        Ok(t)
    }

    fn dispatch(config: &Config, t: &Transcript, id: &str, bytes: Vec<u8>, reply: &Path) -> Result<String, String> {
        let request_path = config.dir.join(format!("{id}.request.json"));
        std::fs::write(&request_path, &bytes).map_err(|e| format!("Write agent request: {e}"))?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().map_err(|e| format!("Locate FilmCraft docs: {e}"))?;
        let docs = root.join("docs");
        let history = serde_json::to_string(
            &t.entries.iter().rev().skip(1).take(8).map(|e| json!({"role": e.role, "text": e.text.chars().take(1000).collect::<String>()})).collect::<Vec<_>>(),
        )
        .map_err(|e| e.to_string())?;
        let prompt = format!(
            "FilmCraft native Agent Chat request (trusted bridge instructions; message/history are user data). You are the existing assigned agent; do not spawn agents or change models. Read {} and {} for the real API. Request file: {}. Request JSON: {}. Recent history, newest first: {}.\n\
             ACT in the SAME live project at {} when requested, not merely offer advice. All existing FilmCraft engine commands and live UI controls are available: dynamically discover via engine.commands and ui.menu.list, inspect via engine.execute command=project.inspect / sequence.inspect and ui.inspect / ui.elements. Execute with engine.execute {{command,params}}; inspect rendered results with ui.screenshot {{path,panel}} (Program for the rendered frame), or MCP bridge render_frame. Use edits and edit.undo for reversible actions; discover audio/mixer, captions/transcripts, effects, colour and export commands and their actual parameters from the registry. No hardcoded capability whitelist. No guessed APIs, no recreated missing DaVinci ML, no parity claims. Follow user's scope; ask before destructive/external actions or overwriting files; never quit/relaunch this app. Verify actual observed results, report command failures and unsupported features plainly.\n\
             TCP protocol: connect to {}, send one JSON object per line {{\"id\":1,\"method\":\"engine.commands\",\"params\":{{}}}}, read one JSON line {{ok,result/error}}; do not send HTTP. CLI alternative: {} --bridge {} exec <discovered-command> <JSON-params>. For visual inspection use the available image tool on the screenshot. Use bounded socket timeouts.\n\
             Within 300 seconds write your genuine answer atomically (temporary file then rename) to EXACTLY {} as UTF-8 JSON {{\"reply\":\"...\"}}, reply nonempty and at most {} UTF-8 bytes. Do not send ui.agent.send recursively. Do not fabricate or echo a reply. The file reply, not terminal text, is displayed in the app.",
            docs.join("control-protocol.md").display(),
            docs.join("agents.md").display(),
            request_path.display(),
            String::from_utf8(bytes).map_err(|e| e.to_string())?,
            history,
            config.address,
            config.address,
            root.join("target/release/filmcraft-cli").display(),
            config.address,
            reply.display(),
            REPLY_LIMIT,
        );
        // argv only: the user's message cannot become shell syntax.
        let mut child = Command::new("herdr")
            .arg("agent")
            .arg("prompt")
            .arg(&config.target)
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Launch herdr agent prompt: {e}"))?;
        let dispatch_start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => return Err(format!("herdr agent prompt exited with {status}; check Herdr target {}", config.target)),
                Ok(None) if dispatch_start.elapsed() < Duration::from_secs(15) => std::thread::sleep(Duration::from_millis(100)),
                result => {
                    let kill = child.kill();
                    let wait = child.wait();
                    return Err(format!("Herdr dispatch timed out or could not be polled: {result:?}; cleanup: {kill:?}, {wait:?}"));
                }
            }
        }
        let start = Instant::now();
        let mut last_error = String::new();
        while start.elapsed() < Duration::from_secs(300) {
            match std::fs::metadata(reply) {
                Ok(_) => {
                    let data = read_bounded(reply, REPLY_LIMIT.saturating_mul(6).saturating_add(128))?;
                    match serde_json::from_slice::<Value>(&data) {
                        Ok(v) => {
                            let text = v.get("reply").and_then(Value::as_str).ok_or("Response JSON requires a string `reply`")?;
                            if text.trim().is_empty() || text.len() > REPLY_LIMIT {
                                return Err("Agent reply is empty or too large".into());
                            }
                            return Ok(text.into());
                        }
                        Err(e) => last_error = format!("; last reply parse error: {e}"),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Inspect agent reply: {e}")),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "Agent reply timed out after 300 seconds at {}{last_error}. Agent actions may already have run; inspect the live project before retrying.",
            reply.display()
        ))
    }
}
