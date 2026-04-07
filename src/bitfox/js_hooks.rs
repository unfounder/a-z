/// JS Security Hooks
///
/// Loads every `.js` file from `<exe_dir>/plugins/` and evaluates each against
/// the current request URL inside a minimal rquickjs context.
///
/// A hook script receives the URL as `globalThis.url` and signals its verdict
/// by calling one of:
///   block()          — URL is blocked; navigation is cancelled.
///   rewrite(newUrl)  — URL is replaced with newUrl before fetching.
///   (no call / allow()) — URL passes through unchanged.
///
/// Example plugin (plugins/no_trackers.js):
///
///   if (url.includes("doubleclick.net")) { block(); }
///   if (url.startsWith("http://")) { rewrite(url.replace("http://", "https://")); }

use crate::error::{BrowserError, BrowserResult};
use rquickjs::{Context, Function, Runtime, Value};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum HookVerdict {
    Allow,
    Block { reason: String },
    Rewrite(String),
}

/// Evaluate all JS plugin files in the `plugins/` directory against `url`.
/// Plugins are evaluated in filesystem order; the first `block()` or `rewrite()`
/// wins and stops further evaluation.
pub fn run_js_security_hooks(url: &str) -> BrowserResult<HookVerdict> {
    let plugin_dir = plugin_dir();
    if !plugin_dir.exists() {
        return Ok(HookVerdict::Allow);
    }

    let scripts = collect_plugin_scripts(&plugin_dir);
    if scripts.is_empty() {
        return Ok(HookVerdict::Allow);
    }

    evaluate_hooks(url, &scripts)
}

fn plugin_dir() -> PathBuf {
    // Place plugins/ next to the executable so they are easy to find.
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("plugins")))
        .unwrap_or_else(|| PathBuf::from("plugins"))
}

fn collect_plugin_scripts(dir: &PathBuf) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("js"))
        .collect();
    paths.sort(); // deterministic order

    for path in paths {
        match std::fs::read_to_string(&path) {
            Ok(code) => {
                let name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                out.push((name, code));
            }
            Err(e) => {
                log::warn!("JS hooks: failed to read {:?}: {}", path, e);
            }
        }
    }
    out
}

fn evaluate_hooks(url: &str, scripts: &[(String, String)]) -> BrowserResult<HookVerdict> {
    // Verdict is written by the JS callbacks via shared state.
    let verdict: std::sync::Arc<Mutex<HookVerdict>> =
        std::sync::Arc::new(Mutex::new(HookVerdict::Allow));

    let verdict_ref = std::sync::Arc::clone(&verdict);
    let url_owned = url.to_string();
    // Clone scripts so the thread closure is 'static.
    let scripts_owned: Vec<(String, String)> = scripts.to_vec();

    let result = std::thread::spawn(move || -> BrowserResult<()> {
        let scripts = scripts_owned;
        let rt = Runtime::new().map_err(|e| BrowserError::JavaScript(e.to_string()))?;
        let ctx = Context::full(&rt).map_err(|e| BrowserError::JavaScript(e.to_string()))?;

        ctx.with(|ctx| {
            // Expose `url` as a global string.
            ctx.globals()
                .set("url", url_owned.as_str())
                .map_err(|e| BrowserError::JavaScript(e.to_string()))?;

            // `block(reason?)` — marks the URL as blocked.
            {
                let v = std::sync::Arc::clone(&verdict_ref);
                ctx.globals()
                    .set(
                        "block",
                        Function::new(ctx.clone(), move |reason: Option<String>| {
                            let msg = reason.unwrap_or_else(|| "blocked by plugin".to_string());
                            *v.lock().unwrap() = HookVerdict::Block { reason: msg };
                        })
                        .map_err(|e| BrowserError::JavaScript(e.to_string()))?,
                    )
                    .map_err(|e| BrowserError::JavaScript(e.to_string()))?;
            }

            // `rewrite(newUrl)` — replaces the URL.
            {
                let v = std::sync::Arc::clone(&verdict_ref);
                ctx.globals()
                    .set(
                        "rewrite",
                        Function::new(ctx.clone(), move |new_url: String| {
                            *v.lock().unwrap() = HookVerdict::Rewrite(new_url);
                        })
                        .map_err(|e| BrowserError::JavaScript(e.to_string()))?,
                    )
                    .map_err(|e| BrowserError::JavaScript(e.to_string()))?;
            }

            // `allow()` — explicit pass-through (no-op, already the default).
            ctx.globals()
                .set(
                    "allow",
                    Function::new(ctx.clone(), || {})
                        .map_err(|e| BrowserError::JavaScript(e.to_string()))?,
                )
                .map_err(|e| BrowserError::JavaScript(e.to_string()))?;

            Ok::<(), BrowserError>(())
        })?;

        for (name, code) in scripts {
            // Stop as soon as a plugin has rendered a verdict.
            {
                let v = verdict_ref.lock().unwrap();
                if !matches!(*v, HookVerdict::Allow) {
                    break;
                }
            }

            ctx.with(|ctx| {
                if let Err(e) = ctx.eval::<Value, _>(code.as_str()) {
                    log::debug!("JS hooks: error in {}: {}", name, e);
                }
            });
        }

        Ok(())
    })
    .join()
    .map_err(|_| BrowserError::JavaScript("JS hook thread panicked".to_string()))?;

    result?;

    let final_verdict = std::sync::Arc::try_unwrap(verdict)
        .map_err(|_| BrowserError::JavaScript("verdict Arc still shared".to_string()))
        .map(|m| m.into_inner().unwrap())?;

    Ok(final_verdict)
}
