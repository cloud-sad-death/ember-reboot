//! blast30 loop rules, carried over: is_per_item_loop, resolve_loop_verb (-exec template or first do-body line).
use regex::Regex;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

pub fn is_per_item_loop(text: &str) -> bool {
    static FIND: OnceLock<Regex> = OnceLock::new();
    static LOOP: OnceLock<Regex> = OnceLock::new();
    re(&FIND, r"(?s)(^|[\s;])find\s.*-exec\s").is_match(text) || re(&LOOP, r"(^|[\s;])(for|while)\s").is_match(text)
}

/// The verb applied to each item: `-exec plutil -p {} \;` -> "plutil -p"; else the first do-body command.
pub fn resolve_loop_verb(body: &str) -> String {
    static EXEC: OnceLock<Regex> = OnceLock::new();
    static DO: OnceLock<Regex> = OnceLock::new();
    static SKIP: OnceLock<Regex> = OnceLock::new();
    static TAIL: OnceLock<Regex> = OnceLock::new();
    let flat = body.replace('\n', " ");
    // sed: greedy .* then `-exec <verb> {}` — the last -exec before a {} wins, as in blast30
    let exec = re(&EXEC, r".*-exec\s+(.*?)\s+\{\}");
    if let Some(c) = exec.captures(&flat) {
        return c[1].trim().to_string();
    }
    let do_re = re(&DO, r"(^|[\s;])do([\s;]|$)");
    let skip = re(&SKIP, r"^\s*((sudo|time|env|command)\s+)*(find|while|for|do|done|if|then|fi|elif|else)([\s;]|$)");
    let tail = re(&TAIL, r#"\s*("?\$[A-Za-z_{][A-Za-z0-9_}]*"?|\{\}).*$"#);
    let mut indo = false;
    let mut lines: Vec<String> = Vec::new();
    for l in body.lines() {
        // one-liners: `for f in *; do cat "$f"; done` — split on ';' so the do-body is its own line
        for part in l.split(';') {
            lines.push(part.to_string());
        }
    }
    for line in &lines {
        if !indo && do_re.is_match(line) {
            indo = true;
            let after = line.trim_start();
            if let Some(rest) = after.strip_prefix("do ") {
                if !rest.trim().is_empty() && !skip.is_match(rest) {
                    return tail.replace(rest.trim(), "").trim().to_string();
                }
            }
            if !skip.is_match(line) && !line.trim().is_empty() {
                return tail.replace(line.trim(), "").trim().to_string();
            }
            continue;
        }
        if indo && !line.trim().is_empty() && !skip.is_match(line) {
            return tail.replace(line.trim(), "").trim().to_string();
        }
    }
    String::new()
}

pub fn first_word(s: &str) -> &str {
    let s = s.trim_start();
    let mut it = s.split_whitespace();
    let w = it.next().unwrap_or("");
    if matches!(w, "sudo" | "command" | "builtin" | "noglob" | "nocorrect") {
        return it.next().unwrap_or(w);
    }
    w
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn detect_and_verbs() {
        let f = r"find /target -name '*.query' -exec cat {} \;";
        assert!(is_per_item_loop(f));
        assert_eq!(resolve_loop_verb(f), "cat");
        let p = "find \"$dir\" -type f -name '*.plist' -exec plutil -p {} \\;";
        assert_eq!(resolve_loop_verb(p), "plutil -p");
        let l = "for f in /x/*.plist; do plutil -p \"$f\"; done";
        assert!(is_per_item_loop(l));
        assert_eq!(resolve_loop_verb(l), "plutil -p");
        let fnb = "\tfind ~/Library/Preferences -name '*.plist' -print0 | while IFS= read -r -d '' f\n\tdo\n\t\tprint -r -- $f\n\t\tplutil -p $f\n\tdone";
        assert!(is_per_item_loop(fnb));
        assert_eq!(resolve_loop_verb(fnb), "print -r --");
        assert!(!is_per_item_loop("ls -la /tmp"));
        assert!(!is_per_item_loop("echo information"));
    }
}
