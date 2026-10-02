// A few lines of a file Claude Code is working on, for the island's session view.
//
// The hook payload carries what an Edit replaces and with what, but not where:
// line numbers and the surrounding lines only exist in the file itself. This
// reads that much and nothing more — local files only, never sent anywhere.

use serde::Serialize;

/// Anything bigger is a generated or binary file nobody wants previewed.
const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_LINES: usize = 40;
const MAX_LINE_LEN: usize = 400;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// 1-based number of `lines[0]`.
    pub start: usize,
    pub lines: Vec<String>,
    /// 1-based line where `needle` starts, when one was given and found.
    pub match_line: Option<usize>,
    pub total_lines: usize,
}

/// With `needle`: `context` lines either side of where it first appears.
/// Without: `limit` lines from line `offset` (1-based).
pub fn read(
    path: &str,
    needle: Option<&str>,
    context: usize,
    offset: usize,
    limit: usize,
) -> Option<Snippet> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return None;
    }
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    // A NUL in the first few KB is as good a binary test as any.
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();

    let (from, to, match_line) = match needle.filter(|n| !n.is_empty()) {
        Some(n) => {
            let at = text.find(n)?;
            let line = text[..at].matches('\n').count(); // 0-based
            let span = n.lines().count().max(1);
            let from = line.saturating_sub(context);
            let to = (line + span + context).min(total);
            (from, to, Some(line + 1))
        }
        None => {
            let from = offset.saturating_sub(1).min(total);
            (from, (from + limit).min(total), None)
        }
    };
    let to = to.min(from + MAX_LINES);

    let lines = all[from..to]
        .iter()
        .map(|l| {
            let l = l.trim_end_matches('\r');
            if l.len() > MAX_LINE_LEN {
                let mut end = MAX_LINE_LEN;
                while !l.is_char_boundary(end) {
                    end -= 1;
                }
                l[..end].to_string()
            } else {
                l.to_string()
            }
        })
        .collect();

    Some(Snippet { start: from + 1, lines, match_line, total_lines: total })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, body: &str) -> String {
        let p = std::env::temp_dir().join(format!("coucou-snippet-{}-{name}", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn finds_the_needle_with_context() {
        let path = temp("a.ts", "l1\nl2\nl3\nconst TVA = 0.196\nl5\nl6\nl7\n");
        let s = read(&path, Some("const TVA = 0.196"), 2, 1, 10).unwrap();
        assert_eq!(s.match_line, Some(4));
        assert_eq!(s.start, 2);
        assert_eq!(s.lines, vec!["l2", "l3", "const TVA = 0.196", "l5", "l6"]);
        assert_eq!(s.total_lines, 7);
    }

    #[test]
    fn reads_a_window_without_a_needle() {
        let path = temp("b.txt", "a\nb\nc\nd\n");
        let s = read(&path, None, 0, 2, 2).unwrap();
        assert_eq!((s.start, s.lines.clone()), (2, vec!["b".to_string(), "c".to_string()]));
    }

    #[test]
    fn refuses_relative_paths_binaries_and_missing_needles() {
        assert!(read("relative.txt", None, 0, 1, 5).is_none());
        let bin = temp("c.bin", "ab\0cd");
        assert!(read(&bin, None, 0, 1, 5).is_none());
        let path = temp("d.txt", "x\ny\n");
        assert!(read(&path, Some("absent"), 2, 1, 5).is_none());
    }
}
