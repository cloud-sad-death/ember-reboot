//! Header pills, in blast's fields and order.
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn field(text: &str, key: &str) -> String {
    text.lines().find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string())).unwrap_or_default()
}

pub fn hostname() -> String {
    let h = run("hostname", &[]);
    if h.is_empty() {
        "localhost".into()
    } else {
        h
    }
}

pub fn pills() -> Vec<(String, String)> {
    let zsh = run("zsh", &["-fc", "print -r -- $ZSH_VERSION"]);
    let host = hostname();
    if cfg!(target_os = "macos") {
        let hw = run("system_profiler", &["-detailLevel", "mini", "SPHardwareDataType"]);
        let model = [field(&hw, "Model Identifier:"), field(&hw, "Model Number:")].join(" ").trim().to_string();
        let memory = format!("{} {}", field(&hw, "Memory:").replace(" GB", "GB"), field(&hw, "Chip:")).trim().to_string();
        vec![
            (run("sw_vers", &["-productName"]), format!("{} {}", run("sw_vers", &["-productVersion"]), run("sw_vers", &["-buildVersion"]))),
            ("MODEL".into(), model),
            ("MEMORY".into(), memory),
            ("HOST".into(), host),
            ("System Firmware Version".into(), field(&hw, "System Firmware Version:")),
            ("ZSH".into(), zsh),
        ]
    } else {
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| field(&m, "MemTotal:").split_whitespace().next().and_then(|k| k.parse::<u64>().ok())).map(|k| format!("{}GB", k / 1_048_576)).unwrap_or_default();
        vec![
            (run("uname", &["-s"]), run("uname", &["-r"])),
            ("MODEL".into(), run("uname", &["-m"])),
            ("MEMORY".into(), mem),
            ("HOST".into(), host),
            ("System Firmware Version".into(), String::new()),
            ("ZSH".into(), zsh),
        ]
    }
}
