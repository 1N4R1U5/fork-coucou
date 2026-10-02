// Coucou for Windows — app wiring and the commands the island calls.

mod claude;
mod files;
mod hooks;
mod integrations;
mod island;
mod log;
mod pipe;
mod secrets;
mod settings;
mod snippet;
mod systime;
mod tray;
mod win_user;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::{ManagerExt, MacosLauncher};

use claude::{Chat, ChatContext, ChatReply};
use files::DroppedFile;
use hooks::{HookPreview, HookStatus};
use island::{PollGate, ScreenInfo};
use pipe::Pending;
use settings::Settings;

/// Keeps spawned helpers from flashing a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Shared {
    pub settings: Mutex<Settings>,
    pub gate: Arc<PollGate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootInfo {
    settings: Settings,
    screen: ScreenInfo,
    version: String,
    hook_path: String,
}

#[tauri::command]
fn boot(app: AppHandle, shared: State<Shared>) -> BootInfo {
    let mut settings = shared.settings.lock().unwrap().clone();
    // The real state of ~/.claude/settings.json wins over whatever we stored.
    settings.hooks_installed = hooks::status().installed;
    let screen = island::screen_info(&app, &settings.screen);
    BootInfo {
        settings,
        screen,
        version: env!("CARGO_PKG_VERSION").to_string(),
        hook_path: settings::hook_exe_path().to_string_lossy().to_string(),
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, shared: State<Shared>, settings: Settings) {
    let (screen_changed, autostart_changed) = {
        let mut current = shared.settings.lock().unwrap();
        let screen_changed = current.screen != settings.screen;
        let autostart_changed = current.autostart != settings.autostart;
        *current = settings.clone();
        (screen_changed, autostart_changed)
    };
    if let Err(err) = settings::save(&settings) {
        eprintln!("[coucou] could not save settings: {err}");
    }
    if autostart_changed {
        let manager = app.autolaunch();
        let result = if settings.autostart { manager.enable() } else { manager.disable() };
        if let Err(err) = result {
            eprintln!("[coucou] autostart: {err}");
        }
    }
    if screen_changed {
        let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
        island::apply_geometry(&app, &settings.screen, collapsed);
    }
    // Keep the other window in step (island ⇄ settings window).
    let _ = app.emit("settings-changed", settings);
}

/// Hidden island → shrink the window to the invisible wake strip and park the
/// cursor poll; anything else → full panel and 60 Hz polling.
#[tauri::command]
fn set_collapsed(app: AppHandle, shared: State<Shared>, collapsed: bool) {
    let pref = shared.settings.lock().unwrap().screen.clone();
    shared.gate.collapsed.store(collapsed, Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed);
    // The wake strip must always take the mouse, and a resize invalidates the flag.
    island::apply_click_through(&app, &shared.gate);
    shared.gate.set_active(!collapsed);
}

/// The front end pushes the island shape; Rust decides click-through from it.
#[tauri::command]
fn set_island_rect(app: AppHandle, shared: State<Shared>, x: f64, y: f64, width: f64, height: f64) {
    shared.gate.set_rect(island::IslandRect { x, y, w: width, h: height });
    #[cfg(unix)]
    island::apply_click_through(&app, &shared.gate);
    #[cfg(windows)]
    let _ = app;
}

#[tauri::command]
fn focus_window(app: AppHandle, focused: bool) {
    let Some(win) = island::window(&app) else { return };
    island::set_activating(&win, focused);
    if focused {
        let _ = win.set_focus();
    }
}

#[tauri::command]
fn reposition(app: AppHandle, shared: State<Shared>) {
    let pref = shared.settings.lock().unwrap().screen.clone();
    let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed);
}

#[tauri::command]
fn open_url(url: String) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(unix)]
    {
        let _ = external("xdg-open").arg(&url).spawn();
    }
}

/// "Open terminal" opens the working folder in VS Code (or VSCodium) when one
/// is installed, and falls back to a terminal, then the file manager.
#[tauri::command]
fn open_in_vscode(path: Option<String>) -> bool {
    // No shell anywhere near this. The path is a project folder chosen by whoever
    // is using Claude Code, and a shell would happily read metacharacters in a
    // folder name as syntax. Finding the launcher ourselves and handing the path
    // over as a separate argument keeps it a path.
    if let Some(mut cmd) = editor_command() {
        if let Some(p) = path.as_deref().filter(|p| !p.is_empty()) {
            cmd.arg(p);
        }
        #[cfg(windows)]
        let spawned = cmd.creation_flags(CREATE_NO_WINDOW).spawn().is_ok();
        #[cfg(unix)]
        let spawned = cmd.spawn().is_ok();
        if spawned {
            return true;
        }
    }
    if let Some(p) = path.as_deref().filter(|p| !p.is_empty()) {
        #[cfg(windows)]
        let _ = Command::new("explorer").arg(p).spawn();
        #[cfg(unix)]
        if !open_terminal_in(p) {
            let _ = external("xdg-open").arg(p).spawn();
        }
    }
    false
}

#[cfg(windows)]
fn editor_command() -> Option<Command> {
    find_on_path("code").map(external)
}

/// VS Code and its open builds, whichever is installed: on $PATH first, then as
/// a Flatpak (VSCodium is often installed that way and puts nothing on $PATH).
#[cfg(unix)]
fn editor_command() -> Option<Command> {
    for name in ["code", "codium", "code-oss"] {
        if let Some(bin) = find_on_path(name) {
            return Some(external(bin));
        }
    }
    let flatpak = find_on_path("flatpak")?;
    for id in ["com.visualstudio.code", "com.vscodium.codium"] {
        let installed = external(&flatpak)
            .args(["info", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if installed {
            let mut cmd = external(&flatpak);
            cmd.args(["run", id]);
            return Some(cmd);
        }
    }
    None
}

/// Opens a terminal emulator in `dir`: $TERMINAL first, then the usual ones.
/// The folder is handed over as the working directory, never through a shell.
#[cfg(unix)]
fn open_terminal_in(dir: &str) -> bool {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.is_empty() {
            candidates.push(t);
        }
    }
    candidates.extend(
        [
            "konsole", "gnome-terminal", "kgx", "xfce4-terminal", "tilix", "alacritty",
            "kitty", "wezterm", "foot", "x-terminal-emulator", "xterm",
        ]
        .map(String::from),
    );
    for name in candidates {
        let Some(bin) = find_on_path(&name) else { continue };
        let mut cmd = external(&bin);
        cmd.current_dir(dir);
        // These two talk to an already-running server, which ignores our cwd.
        match name.as_str() {
            "gnome-terminal" | "kgx" => {
                cmd.arg(format!("--working-directory={dir}"));
            }
            _ => {}
        }
        if cmd.spawn().is_ok() {
            return true;
        }
    }
    false
}

/// Set when `run()` forced XWayland for the island; programs we launch must not
/// inherit that choice.
#[cfg(unix)]
static FORCED_X11: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A command for a program outside Coucou (browser, editor, terminal, file
/// manager). Inside an AppImage the environment points library, plugin and
/// interpreter paths into our own mount, and a Qt or Python app started with
/// those either crashes or silently fails to open. Strip every entry that lives
/// under $APPDIR, and the backend we forced, so the program sees the user's
/// normal session.
#[cfg(unix)]
fn external(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if FORCED_X11.load(Ordering::Relaxed) {
        cmd.env_remove("GDK_BACKEND");
    }
    if let Some(appdir) = std::env::var_os("APPDIR").and_then(|a| a.into_string().ok()) {
        let appdir = appdir.trim_end_matches('/').to_string();
        if !appdir.is_empty() {
            for (key, value) in std::env::vars() {
                if !value.contains(&appdir) {
                    continue;
                }
                let kept: Vec<&str> = value
                    .split(':')
                    .filter(|part| !part.is_empty() && !part.starts_with(&appdir))
                    .collect();
                if kept.is_empty() {
                    cmd.env_remove(&key);
                } else {
                    cmd.env(&key, kept.join(":"));
                }
            }
            for key in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
                cmd.env_remove(key);
            }
        }
    }
    cmd
}

#[cfg(windows)]
fn external(program: impl AsRef<std::ffi::OsStr>) -> Command {
    Command::new(program)
}

/// What coucou-hook saw of the terminal around a Claude Code session.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(windows, allow(dead_code))]
struct TerminalRef {
    konsole_service: Option<String>,
    konsole_window: Option<String>,
    konsole_session: Option<String>,
    flatpak_id: Option<String>,
    ancestor_pids: Option<Vec<u32>>,
}

/// The ↗ button on a Claude Code session: bring back the window it runs in —
/// on KDE the exact Konsole tab — and only open something new when that
/// window cannot be found.
#[tauri::command]
fn focus_terminal(terminal: Option<TerminalRef>, path: Option<String>) -> bool {
    #[cfg(unix)]
    {
        let terminal = terminal.unwrap_or_default();
        // D-Bus calls and the KWin script take a moment; never on the UI thread.
        std::thread::spawn(move || {
            let found = focus_terminal_linux(&terminal, path.as_deref());
            crate::log::line(format!(
                "focus terminal: konsole={} flatpak={} pids={} → {}",
                terminal.konsole_session.as_deref().unwrap_or("-"),
                terminal.flatpak_id.as_deref().unwrap_or("-"),
                terminal.ancestor_pids.as_ref().map_or(0, Vec::len),
                if found { "raised" } else { "fallback" },
            ));
            if !found {
                open_in_vscode(path);
            }
        });
        true
    }
    #[cfg(windows)]
    {
        let _ = terminal;
        open_in_vscode(path)
    }
}

#[cfg(unix)]
fn focus_terminal_linux(t: &TerminalRef, path: Option<&str>) -> bool {
    // Inside a Flatpak (VSCodium…) the pids are the sandbox's own and mean
    // nothing out here; asking the app to open the folder raises its window.
    if let Some(id) = t.flatpak_id.as_deref().filter(|id| is_app_id(id)) {
        if let Some(flatpak) = find_on_path("flatpak") {
            let mut cmd = external(flatpak);
            cmd.args(["run", id]);
            if let Some(p) = path.filter(|p| !p.is_empty()) {
                cmd.arg(p);
            }
            return cmd.spawn().is_ok();
        }
    }

    let mut pids: Vec<u32> = Vec::new();
    if let (Some(service), Some(window), Some(session)) = (
        t.konsole_service.as_deref(),
        t.konsole_window.as_deref(),
        t.konsole_session.as_deref(),
    ) {
        let valid = (service.starts_with(':') || service.starts_with("org.kde.konsole"))
            && service.chars().all(|c| c.is_ascii_alphanumeric() || ".:-_".contains(c))
            && window.starts_with("/Windows/")
            && window[9..].chars().all(|c| c.is_ascii_digit());
        let session_id = session
            .strip_prefix("/Sessions/")
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if let (true, Some(n)) = (valid, session_id) {
            let _ = gdbus(&[
                "--dest", service, "--object-path", window,
                "--method", "org.kde.konsole.Window.setCurrentSession", n,
            ]);
            if let Some(out) = gdbus(&[
                "--dest", "org.freedesktop.DBus", "--object-path", "/org/freedesktop/DBus",
                "--method", "org.freedesktop.DBus.GetConnectionUnixProcessID", service,
            ]) {
                // "(uint32 4822,)"
                if let Some(pid) = out
                    .split(|c: char| !c.is_ascii_digit())
                    .filter(|s| !s.is_empty())
                    .last()
                    .and_then(|s| s.parse().ok())
                {
                    pids.push(pid);
                }
            }
        }
    }
    if pids.is_empty() {
        pids = t.ancestor_pids.clone().unwrap_or_default();
    }
    pids.retain(|p| *p > 1);
    !pids.is_empty() && kwin_activate(&pids)
}

#[cfg(unix)]
fn is_app_id(id: &str) -> bool {
    id.contains('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

#[cfg(unix)]
fn gdbus(args: &[&str]) -> Option<String> {
    let out = external("gdbus")
        .args(["call", "--session"])
        .args(args)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Asks KWin to activate the first normal window owned by one of `pids`.
/// Wayland gives clients no way to raise another app's window; the compositor
/// can, through a one-off script. Other desktops simply say no and we fall back.
#[cfg(unix)]
fn kwin_activate(pids: &[u32]) -> bool {
    const NAME: &str = "coucou-raise";
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let script = format!(
        "const pids = [{list}];\n\
         const all = workspace.windowList ? workspace.windowList() : workspace.clientList();\n\
         let hit = null;\n\
         for (const pid of pids) {{\n\
           for (const w of all) {{\n\
             if (w.pid === pid && w.normalWindow) {{ hit = w; break; }}\n\
           }}\n\
           if (hit) break;\n\
         }}\n\
         if (hit) {{\n\
           hit.minimized = false;\n\
           if ('activeWindow' in workspace) workspace.activeWindow = hit;\n\
           else workspace.activeClient = hit;\n\
         }}\n"
    );
    let path = pipe::socket_path().with_file_name("raise.js");
    if std::fs::write(&path, script).is_err() {
        return false;
    }
    let path_str = path.to_string_lossy().into_owned();
    let _ = gdbus(&[
        "--dest", "org.kde.KWin", "--object-path", "/Scripting",
        "--method", "org.kde.kwin.Scripting.unloadScript", NAME,
    ]);
    let Some(out) = gdbus(&[
        "--dest", "org.kde.KWin", "--object-path", "/Scripting",
        "--method", "org.kde.kwin.Scripting.loadScript", &path_str, NAME,
    ]) else {
        return false;
    };
    // "(0,)" — a negative id means KWin refused the script.
    let Some(id) = out
        .trim_matches(|c: char| c == '(' || c == ')' || c == ',' || c.is_whitespace())
        .parse::<i64>()
        .ok()
        .filter(|id| *id >= 0)
    else {
        return false;
    };
    let ran = gdbus(&[
        "--dest", "org.kde.KWin", "--object-path", &format!("/Scripting/Script{id}"),
        "--method", "org.kde.kwin.Script.run",
    ])
    .is_some();
    // Give the script its turn before taking it away again.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = gdbus(&[
        "--dest", "org.kde.KWin", "--object-path", "/Scripting",
        "--method", "org.kde.kwin.Scripting.unloadScript", NAME,
    ]);
    ran
}

/// Our own `where`: walks %PATH% against %PATHEXT%, no shell involved.
/// Rust quotes arguments correctly for `.cmd`/`.bat` targets since 1.77, so
/// spawning `code.cmd` directly is safe.
#[cfg(windows)]
fn find_on_path(stem: &str) -> Option<std::path::PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{stem}{}", ext.to_lowercase()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Our own `which`: the first entry on $PATH that is a regular file.
#[cfg(unix)]
fn find_on_path(stem: &str) -> Option<std::path::PathBuf> {
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        let candidate = dir.join(stem);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Tray → Pause. Paused means paused: the pollers stop talking to the network,
/// not just the island stopping showing things.
#[tauri::command]
fn set_paused(paused: bool) {
    integrations::set_paused(paused);
}

// ── Claude Code hooks ─────────────────────────────────────────────────────────

#[tauri::command]
fn hooks_status() -> HookStatus {
    hooks::status()
}

/// Returns the diff the user has to look at before anything is written.
#[tauri::command]
fn hooks_preview(install: bool) -> Result<HookPreview, String> {
    hooks::preview(install)
}

/// Only ever called from an explicit click in the settings window.
#[tauri::command]
fn hooks_apply(
    app: AppHandle,
    shared: State<Shared>,
    install: bool,
    fingerprint: String,
) -> Result<String, String> {
    // The fingerprint comes from the preview the user actually looked at, so a
    // settings.json that changed in between is refused rather than overwritten.
    let backup = hooks::write(install, &fingerprint)?;
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        current.hooks_installed = install;
        let _ = settings::save(&current);
        current.clone()
    };
    let _ = app.emit("settings-changed", updated);
    Ok(backup)
}

#[tauri::command]
fn approval_decision(app: AppHandle, request_id: String, decision: String) {
    pipe::answer(&app, &request_id, &decision);
}

/// The island has the card on screen, so the long wait for a human may begin.
/// Until this arrives the relay only waits a few hundred milliseconds, which is
/// what stops a paused or unresponsive island from freezing Claude Code.
#[tauri::command]
fn approval_ack(app: AppHandle, request_id: String) {
    pipe::acknowledge(&app, &request_id);
}

/// Nobody can act on this request — the island is paused, or another card is
/// already up. Claude Code falls back to asking in the terminal immediately.
#[tauri::command]
fn approval_decline(app: AppHandle, request_id: String) {
    pipe::decline(&app, &request_id);
}

// ── Chat, files and secrets ───────────────────────────────────────────────────

/// One chat turn. The API key and any file bytes stay on the Rust side.
#[tauri::command]
async fn chat_send(
    shared: State<'_, Shared>,
    chat: State<'_, Chat>,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let model = shared.settings.lock().unwrap().model.clone();
    claude::send(&chat, &model, query, context).await
}

#[tauri::command]
fn chat_reset(chat: State<Chat>) {
    chat.reset();
}

/// Copies a dropped file into the inbox and reports its name back.
#[tauri::command]
fn ingest_file(path: String) -> Result<DroppedFile, String> {
    files::ingest(&path)
}

/// The island may only ask whether a key exists — never read it.
#[tauri::command]
fn secret_present(key: String) -> bool {
    secrets::present(&key)
}

#[tauri::command]
fn secret_set(key: String, value: String) -> Result<(), String> {
    secrets::set(&key, &value)
}

#[tauri::command]
fn secret_clear(key: String) -> Result<(), String> {
    secrets::clear(&key)
}

/// Opens the configured n8n instance — the URL lives in the Credential Manager.
#[tauri::command]
fn open_n8n() {
    if let Some(url) = secrets::get("n8n-url") {
        open_url(url);
    }
}

/// Refresh buttons in the integration cards.
#[tauri::command]
async fn refresh_integration(app: AppHandle, id: String) {
    integrations::poll_once(app, &id).await;
}

/// A few lines of a file Claude Code is reading or editing, for the session view.
#[tauri::command]
fn read_snippet(
    path: String,
    needle: Option<String>,
    context: Option<usize>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Option<snippet::Snippet> {
    snippet::read(
        &path,
        needle.as_deref(),
        context.unwrap_or(2),
        offset.unwrap_or(1),
        limit.unwrap_or(12),
    )
}

/// Lets the island write to the same log as the Rust side.
#[tauri::command]
fn log_line(message: String) {
    log::line(format!("ui  {message}"));
}

// ── Settings window ───────────────────────────────────────────────────────────

/// WebView2 allows exactly one browser environment per app, and its options are
/// fixed by whichever webview is created first. Every window must therefore ask
/// for the *same* arguments as the island (see `additionalBrowserArgs` in
/// tauri.conf.json) — a mismatch makes the second window come up blank, with no
/// error anywhere.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

/// In a dev build the pages are served by Vite, so the second window needs the
/// absolute dev URL; a bundled build resolves it inside the app bundle.
fn settings_page_url(app: &AppHandle) -> WebviewUrl {
    #[cfg(dev)]
    if let Some(mut base) = app.config().build.dev_url.clone() {
        base.set_path("/settings.html");
        return WebviewUrl::External(base);
    }
    let _ = app;
    WebviewUrl::App("settings.html".into())
}

/// The settings window is created hidden at launch and only ever shown and
/// hidden afterwards. A WebView2 window created later — on the main thread or
/// not — silently comes up blank in this app, so the window that works is the
/// one that exists before the island's webview does.
fn create_settings_window(app: &AppHandle) {
    let url = settings_page_url(app);
    match WebviewWindowBuilder::new(app, "settings", url)
        .additional_browser_args(BROWSER_ARGS)
        .title("Settings — Coucou")
        .inner_size(560.0, 680.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .visible(false)
        .center()
        .build()
    {
        Ok(win) => {
            // Closing it must only hide it, or it could never be reopened.
            let hidden = win.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = hidden.hide();
                }
            });
        }
        Err(err) => log::line(format!("settings window failed: {err}")),
    }
}

pub fn show_settings_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("settings") else {
        log::line("settings window missing");
        return;
    };
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}

#[tauri::command]
fn open_settings_window(app: AppHandle) {
    show_settings_window(&app);
}

pub fn run() {
    // On a Wayland session a client cannot place its own top-level window, so the
    // compositor drops the island in the centre instead of at the top edge, and
    // the global cursor position needed for the peek/click-through is unreadable.
    // Running through XWayland restores both. Done before any GTK code touches the
    // display, and only when the user has not chosen a backend themselves.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("GDK_BACKEND").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            std::env::set_var("GDK_BACKEND", "x11");
            FORCED_X11.store(true, Ordering::Relaxed);
        }
    }

    let loaded = settings::load();
    let gate = Arc::new(PollGate::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = app.emit_to(island::WINDOW_LABEL, "tray", "open".to_string());
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .manage(Shared {
            settings: Mutex::new(loaded.clone()),
            gate: gate.clone(),
        })
        .manage(Pending::default())
        .manage(Chat::default())
        .invoke_handler(tauri::generate_handler![
            boot,
            save_settings,
            set_collapsed,
            set_island_rect,
            focus_window,
            reposition,
            open_url,
            focus_terminal,
            read_snippet,
            open_in_vscode,
            quit_app,
            hooks_status,
            hooks_preview,
            hooks_apply,
            approval_decision,
            approval_ack,
            approval_decline,
            log_line,
            chat_send,
            chat_reset,
            ingest_file,
            secret_present,
            secret_set,
            secret_clear,
            refresh_integration,
            open_n8n,
            open_settings_window,
            set_paused,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            tray::build(&handle)?;
            // Before the island: see create_settings_window.
            create_settings_window(&handle);

            if let Some(win) = island::window(&handle) {
                island::make_non_activating(&win);
                island::apply_geometry(&handle, &loaded.screen, false);
                let _ = win.show();
            }
            gate.collapsed.store(false, Ordering::Relaxed);
            gate.set_active(true);
            island::spawn_cursor_poll(handle.clone(), gate.clone());

            log::line(format!("--- Coucou {} started ---", env!("CARGO_PKG_VERSION")));
            hooks::ensure_hook_exe(&handle);
            pipe::start(handle.clone());
            integrations::start(handle.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Coucou");
}
