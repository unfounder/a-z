use super::{RenderBackend, RenderOutput, RgbaImage};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tiny_skia::Pixmap;
use uuid::Uuid;

const SERVO_CMD_ENV: &str = "VULTURINE_SERVO_CMD_JSON";
const LADYBIRD_CMD_ENV: &str = "VULTURINE_LADYBIRD_CMD_JSON";
const SERVO_BIN_ENV: &str = "VULTURINE_SERVO_BIN";
const LADYBIRD_BIN_ENV: &str = "VULTURINE_LADYBIRD_BIN";

#[derive(Clone, Copy, Debug)]
pub enum ExternalKind {
    Servo,
    Ladybird,
}

impl ExternalKind {
    fn slug(self) -> &'static str {
        match self {
            ExternalKind::Servo => "servo",
            ExternalKind::Ladybird => "ladybird",
        }
    }

    fn command_env(self) -> &'static str {
        match self {
            ExternalKind::Servo => SERVO_CMD_ENV,
            ExternalKind::Ladybird => LADYBIRD_CMD_ENV,
        }
    }

    fn bin_env(self) -> &'static str {
        match self {
            ExternalKind::Servo => SERVO_BIN_ENV,
            ExternalKind::Ladybird => LADYBIRD_BIN_ENV,
        }
    }
}

static SERVO_CANDIDATES: OnceLock<Vec<Vec<String>>> = OnceLock::new();
static LADYBIRD_CANDIDATES: OnceLock<Vec<Vec<String>>> = OnceLock::new();

pub fn is_configured(backend: RenderBackend) -> bool {
    match backend {
        RenderBackend::Software => true,
        RenderBackend::Servo => !load_command_candidates(ExternalKind::Servo).is_empty(),
        RenderBackend::Ladybird => !load_command_candidates(ExternalKind::Ladybird).is_empty(),
    }
}

pub fn config_hint(backend: RenderBackend) -> Option<&'static str> {
    match backend {
        RenderBackend::Software => None,
        RenderBackend::Servo => Some(SERVO_CMD_ENV),
        RenderBackend::Ladybird => Some(LADYBIRD_CMD_ENV),
    }
}

pub fn try_render(kind: ExternalKind, url: &str, width: u32, height: u32) -> Option<RenderOutput> {
    let candidates = load_command_candidates(kind);
    if candidates.is_empty() {
        return None;
    }

    let mut first_error: Option<String> = None;
    for template in candidates {
        match run_external_template(template, kind, url, width, height) {
            Ok(render) => return Some(render),
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }

    if let Some(err) = first_error {
        log::warn!(
            "Vulturine/{:?}: external render failed with all templates: {}",
            kind,
            err
        );
    }
    None
}

fn load_command_candidates(kind: ExternalKind) -> &'static Vec<Vec<String>> {
    let cell = match kind {
        ExternalKind::Servo => &SERVO_CANDIDATES,
        ExternalKind::Ladybird => &LADYBIRD_CANDIDATES,
    };
    cell.get_or_init(|| build_command_candidates(kind))
}

fn build_command_candidates(kind: ExternalKind) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(env_cmd) = parse_command_json(kind.command_env()) {
        push_unique_template(&mut out, &mut seen, env_cmd);
    }

    if let Ok(bin_path) = std::env::var(kind.bin_env()) {
        if !bin_path.trim().is_empty() {
            for t in templates_for_executable(kind, bin_path.trim()) {
                push_unique_template(&mut out, &mut seen, t);
            }
        }
    }

    if let Some(found) = auto_discover_executable(kind) {
        for t in templates_for_executable(kind, &found) {
            push_unique_template(&mut out, &mut seen, t);
        }
    }

    if out.is_empty() {
        log::debug!(
            "Vulturine/{:?}: no external backend command found (set {} or {})",
            kind,
            kind.command_env(),
            kind.bin_env()
        );
    } else {
        log::info!(
            "Vulturine/{:?}: loaded {} external command template(s)",
            kind,
            out.len()
        );
    }

    out
}

fn parse_command_json(env_name: &str) -> Option<Vec<String>> {
    let raw = std::env::var(env_name).ok()?;
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(err) => {
            log::warn!("Vulturine: invalid JSON in {}: {}", env_name, err);
            return None;
        }
    };

    let items = parsed.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(s) = item.as_str() else {
            return None;
        };
        out.push(s.to_string());
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn templates_for_executable(kind: ExternalKind, exe: &str) -> Vec<Vec<String>> {
    match kind {
        ExternalKind::Servo => vec![
            vec![
                exe.to_string(),
                "--headless".to_string(),
                "--exit".to_string(),
                "--output".to_string(),
                "{out}".to_string(),
                "--window-size".to_string(),
                "{width}x{height}".to_string(),
                "{url}".to_string(),
            ],
            vec![
                exe.to_string(),
                "--headless".to_string(),
                "--exit".to_string(),
                "--output".to_string(),
                "{out}".to_string(),
                "--screen-size".to_string(),
                "{width}x{height}".to_string(),
                "{url}".to_string(),
            ],
            vec![
                exe.to_string(),
                "--headless".to_string(),
                "--exit".to_string(),
                "--output".to_string(),
                "{out}".to_string(),
                "{url}".to_string(),
            ],
        ],
        ExternalKind::Ladybird => vec![
            vec![
                exe.to_string(),
                "--headless".to_string(),
                "--url".to_string(),
                "{url}".to_string(),
                "--width".to_string(),
                "{width}".to_string(),
                "--height".to_string(),
                "{height}".to_string(),
                "--output".to_string(),
                "{out}".to_string(),
            ],
            vec![
                exe.to_string(),
                "--url".to_string(),
                "{url}".to_string(),
                "--screenshot".to_string(),
                "{out}".to_string(),
                "--size".to_string(),
                "{width}x{height}".to_string(),
            ],
            vec![
                exe.to_string(),
                "{url}".to_string(),
                "--screenshot".to_string(),
                "{out}".to_string(),
            ],
        ],
    }
}

fn push_unique_template(
    out: &mut Vec<Vec<String>>,
    seen: &mut HashSet<String>,
    template: Vec<String>,
) {
    if template.is_empty() {
        return;
    }
    let key = template.join("\u{1f}");
    if seen.insert(key) {
        out.push(template);
    }
}

fn auto_discover_executable(kind: ExternalKind) -> Option<String> {
    let mut names: Vec<&str> = match kind {
        ExternalKind::Servo => vec!["servo", "servoshell", "servo-headless", "servo_shell"],
        ExternalKind::Ladybird => vec![
            "ladybird",
            "ladybird-headless",
            "ladybird_app",
            "ladybird-webcontent",
        ],
    };

    #[cfg(target_os = "windows")]
    {
        match kind {
            ExternalKind::Servo => {
                let extra = [r"C:\Program Files\Servo\servo.exe", r"C:\servo\servo.exe"];
                for p in extra {
                    if Path::new(p).is_file() {
                        return Some(p.to_string());
                    }
                }
            }
            ExternalKind::Ladybird => {
                let extra = [
                    r"C:\Program Files\Ladybird\ladybird.exe",
                    r"C:\ladybird\ladybird.exe",
                ];
                for p in extra {
                    if Path::new(p).is_file() {
                        return Some(p.to_string());
                    }
                }
            }
        }
    }

    // Keep deterministic order.
    names.sort_unstable();
    names.dedup();
    find_executable_in_path(&names).map(|p| p.to_string_lossy().to_string())
}

fn find_executable_in_path(names: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let path_dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();

    #[cfg(target_os = "windows")]
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();

    for dir in &path_dirs {
        for name in names {
            let base = Path::new(name);
            if base.extension().is_some() {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
                continue;
            }

            #[cfg(target_os = "windows")]
            {
                for ext in &exts {
                    let candidate = dir.join(format!("{}{}", name, ext));
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }

            #[cfg(not(target_os = "windows"))]
            {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn run_external_template(
    template: &[String],
    kind: ExternalKind,
    url: &str,
    width: u32,
    height: u32,
) -> Result<RenderOutput, String> {
    let out_path = std::env::temp_dir().join(format!(
        "vulturine-{}-{}-{}x{}.png",
        kind.slug(),
        Uuid::new_v4(),
        width,
        height
    ));
    let out_str = out_path.to_string_lossy();
    let width_s = width.to_string();
    let height_s = height.to_string();

    let uses_out_file = template.iter().any(|part| part.contains("{out}"));
    let resolved: Vec<String> = template
        .iter()
        .map(|part| {
            part.replace("{url}", url)
                .replace("{width}", &width_s)
                .replace("{height}", &height_s)
                .replace("{out}", &out_str)
        })
        .collect();

    if resolved.is_empty() {
        return Err("empty command template".to_string());
    }

    let mut cmd = Command::new(&resolved[0]);
    if resolved.len() > 1 {
        cmd.args(&resolved[1..]);
    }
    let output = cmd
        .output()
        .map_err(|err| format!("spawn failed for '{}': {}", resolved[0], err))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "template '{}' exited {}: {}",
            resolved[0],
            output.status,
            truncate_for_log(stderr.as_ref(), 200)
        ));
    }

    if uses_out_file {
        let pixmap = Pixmap::load_png(&out_path)
            .map_err(|err| format!("failed to decode PNG '{}': {}", out_path.display(), err))?;
        let _ = fs::remove_file(&out_path);
        return Ok(render_from_pixmap(pixmap));
    }

    if output.stdout.is_empty() {
        return Err("command produced no PNG bytes on stdout".to_string());
    }

    let pixmap = Pixmap::decode_png(&output.stdout)
        .map_err(|err| format!("failed to decode PNG stdout: {}", err))?;
    Ok(render_from_pixmap(pixmap))
}

fn render_from_pixmap(pixmap: Pixmap) -> RenderOutput {
    let w = pixmap.width();
    let h = pixmap.height();
    let pixels: Vec<[u8; 4]> = pixmap
        .data()
        .chunks_exact(4)
        .map(|px| [px[0], px[1], px[2], px[3]])
        .collect();

    RenderOutput {
        image: RgbaImage {
            size: [w as usize, h as usize],
            pixels,
        },
        width: w,
        height: h,
        hitboxes: Vec::new(),
    }
}

fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars + 3);
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}
