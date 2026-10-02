// Chat through Claude Code (`claude -p`) when no API key is set, so the chat
// runs on the user's Claude subscription instead of pay-as-you-go API credit.
//
// Each turn is one `claude -p` run; `--resume` carries the conversation. The
// session is kept away from everything else Claude Code does on this machine:
// * it runs in the inbox, where the dropped files are, never in a project;
// * `--setting-sources project` skips the user's settings, and with them the
//   hooks Coucou installed, so the chat never shows up as a session in the
//   island (COUCOU_CHAT also makes coucou-hook stand down, should one run);
// * only Read, WebSearch and WebFetch exist: it can read the files and look
//   things up, never run a command or write anything.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;

use crate::claude::{Chat, ChatContext, ChatReply, SYSTEM_PROMPT};
use crate::files;

const TOOLS: &str = "Read,WebSearch,WebFetch";
const TIMEOUT: Duration = Duration::from_secs(180);

/// `claude` on $PATH, or where its installers put it. A desktop launch often
/// has a PATH without ~/.local/bin, so the usual spots are tried too.
pub fn find() -> Option<PathBuf> {
    if let Some(found) = std::env::var_os("PATH").and_then(|dirs| {
        std::env::split_paths(&dirs).map(|d| d.join("claude")).find(|p| p.is_file())
    }) {
        return Some(found);
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        ".local/bin/claude",
        ".claude/local/claude",
        ".npm-global/bin/claude",
        ".bun/bin/claude",
        ".volta/bin/claude",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .chain(["/usr/local/bin/claude", "/usr/bin/claude"].map(PathBuf::from))
    .find(|p| p.is_file())
}

/// Turns the dropped files or window into the first message's preamble.
fn preamble(context: &Option<ChatContext>) -> (String, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut add = |path: &str| {
        if let Some(parent) = Path::new(path).parent() {
            if !dirs.iter().any(|d: &PathBuf| d == parent) {
                dirs.push(parent.to_path_buf());
            }
        }
    };
    let text = match context {
        Some(ChatContext::File { name, path }) => {
            add(path);
            format!("The user dropped a file. Read it before answering.\n- {name}: {path}\n\n")
        }
        Some(ChatContext::Files { files }) => {
            let mut t = String::from("The user dropped these files. Read them before answering.\n");
            for f in files {
                add(&f.path);
                t.push_str(&format!("- {}: {}\n", f.name, f.path));
            }
            t.push('\n');
            t
        }
        Some(ChatContext::Window { app_name, title, url }) => {
            let mut t = format!("Context — App: {app_name}, Window: {title}");
            if let Some(url) = url {
                t.push_str(&format!(", URL: {url}"));
            }
            t.push_str("\n\n");
            t
        }
        None => String::new(),
    };
    (text, dirs)
}

pub async fn send(chat: &Chat, query: String, context: Option<ChatContext>) -> Result<ChatReply, String> {
    let bin = find().ok_or_else(|| {
        "No API key and Claude Code not found. Install Claude Code or add a key in Settings.".to_string()
    })?;

    let workdir = files::inbox_dir();
    std::fs::create_dir_all(&workdir).map_err(|e| e.to_string())?;

    let resume = chat.cli_session();
    // Context only rides along with the first turn; the session remembers it.
    let (pre, dirs) = if resume.is_none() { preamble(&context) } else { (String::new(), Vec::new()) };

    let mut cmd = crate::external(&bin);
    cmd.current_dir(&workdir)
        .env("COUCOU_CHAT", "1")
        .arg("-p")
        .arg(format!("{pre}{query}"))
        .args(["--output-format", "json", "--setting-sources", "project"])
        .args(["--tools", TOOLS, "--allowedTools", TOOLS])
        .args(["--append-system-prompt", SYSTEM_PROMPT]);
    for dir in dirs.iter().filter(|d| **d != workdir) {
        cmd.arg("--add-dir").arg(dir);
    }
    if let Some(id) = &resume {
        cmd.args(["--resume", id]);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    let output = match tokio::time::timeout(TIMEOUT, cmd.output()).await {
        Err(_) => return Err("Claude Code took too long to answer.".into()),
        Ok(Err(e)) => return Err(format!("Could not start Claude Code: {e}")),
        Ok(Ok(out)) => out,
    };

    let parsed: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    let Some(v) = parsed else {
        let err = String::from_utf8_lossy(&output.stderr);
        let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
        crate::log::line(format!("claude -p failed: {line}"));
        return Err(format!("Claude Code: {line}"));
    };

    let text = v.get("result").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if v.get("is_error").and_then(Value::as_bool) == Some(true) {
        // Typically "not logged in" or a usage limit: Claude Code's own words.
        return Err(if text.is_empty() { "Claude Code returned an error.".into() } else { text });
    }
    if let Some(id) = v.get("session_id").and_then(Value::as_str) {
        chat.set_cli_session(id.to_string());
    }
    if text.is_empty() {
        return Err("No response text.".into());
    }
    Ok(ChatReply { text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::FileRef;

    #[test]
    fn preamble_lists_every_file_and_its_folder_once() {
        let ctx = Some(ChatContext::Files {
            files: vec![
                FileRef { name: "a.pdf".into(), path: "/inbox/a.pdf".into() },
                FileRef { name: "b.png".into(), path: "/inbox/b.png".into() },
                FileRef { name: "c.txt".into(), path: "/home/me/c.txt".into() },
            ],
        });
        let (text, dirs) = preamble(&ctx);
        assert!(text.contains("- a.pdf: /inbox/a.pdf") && text.contains("- c.txt: /home/me/c.txt"));
        assert_eq!(dirs, vec![PathBuf::from("/inbox"), PathBuf::from("/home/me")]);
        assert_eq!(preamble(&None).0, "");
    }
}

/// Real `claude -p` round trip, on the subscription: `cargo test -- --ignored`.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn asks_about_a_dropped_file_then_follows_up() {
        let src = std::env::temp_dir().join("coucou-live-note.txt");
        std::fs::write(&src, "The secret word is papaya.").unwrap();
        let file = files::ingest(src.to_str().unwrap()).unwrap();
        let chat = Chat::default();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let ctx = Some(ChatContext::File { name: file.name.clone(), path: file.path.clone() });
        let first = rt.block_on(send(&chat, "What is the secret word? One word.".into(), ctx.clone())).unwrap();
        assert!(first.text.to_lowercase().contains("papaya"), "{}", first.text);
        let second = rt.block_on(send(&chat, "Spell it backwards. One word.".into(), ctx)).unwrap();
        assert!(second.text.to_lowercase().contains("ayapap"), "{}", second.text);
        let _ = std::fs::remove_file(&file.path);
    }
}
