//! `veer serve`: the helper protocol the Mac app speaks.
//!
//! One JSON object per line in each direction. Requests carry an `id`;
//! each gets exactly one response with the same `id` (`result` or
//! `error`). A `run` request answers at once with a job number, then
//! streams events tagged with that job until `job_finished`.
//!
//! When stdin closes (the app quit or crashed), running jobs are cancelled
//! and the helper exits, so it can never outlive its app.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use veer_core::job::{self, Executor, RunOptions};
use veer_core::{catalog, format, Action, Capabilities, Recipe};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Deserialize)]
struct Request {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectParams {
    paths: Vec<PathBuf>,
    #[serde(default)]
    recipes_dir: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunParams {
    paths: Vec<PathBuf>,
    action: Action,
    #[serde(default)]
    options: RunOptions,
    /// Run each unit in its own worker process (default true).
    #[serde(default = "yes")]
    isolate: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsParams {
    paths: Vec<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelParams {
    job: u64,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RecipesParams {
    #[serde(default)]
    recipes_dir: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRecipeParams {
    recipe: Recipe,
    #[serde(default)]
    recipes_dir: Option<PathBuf>,
    /// An existing recipe file to overwrite (editing), instead of a new one.
    #[serde(default)]
    replace: Option<PathBuf>,
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(out: &Out, v: &Value) {
    let mut o = out.lock().expect("stdout lock");
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn error_response(id: &Value, code: &str, message: impl Into<String>) -> Value {
    json!({ "id": id, "error": { "code": code, "message": message.into() } })
}

fn all_recipes(dir: Option<&PathBuf>) -> (Vec<Recipe>, Vec<Value>) {
    let mut recipes = Recipe::builtin();
    let mut errors = Vec::new();
    if let Some(d) = dir.cloned().or_else(crate::default_recipes_dir) {
        let (mut found, errs) = Recipe::load_dir(&d);
        recipes.append(&mut found);
        errors = errs
            .into_iter()
            .map(|(p, e)| json!({ "path": p, "message": e }))
            .collect();
    }
    (recipes, errors)
}

pub fn serve(
    input: impl BufRead,
    output: Box<dyn Write + Send>,
    caps: &'static Capabilities,
    worker: Option<(PathBuf, Vec<String>)>,
) -> i32 {
    let out: Out = Arc::new(Mutex::new(output));
    let jobs: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>> = Arc::default();
    let next_job = AtomicU64::new(1);
    let mut threads = Vec::new();

    for line in input.lines() {
        let Ok(line) = line else { break };
        threads.retain(|t: &std::thread::JoinHandle<()>| !t.is_finished());
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                send(
                    &out,
                    &error_response(&Value::Null, "bad_request", e.to_string()),
                );
                continue;
            }
        };
        let id = req.id.clone();
        macro_rules! params {
            ($t:ty) => {
                match serde_json::from_value::<$t>(if req.params.is_null() {
                    json!({})
                } else {
                    req.params.clone()
                }) {
                    Ok(p) => p,
                    Err(e) => {
                        send(&out, &error_response(&id, "bad_params", e.to_string()));
                        continue;
                    }
                }
            };
        }
        match req.method.as_str() {
            "hello" => send(
                &out,
                &json!({ "id": id, "result": {
                    "version": veer_core::VERSION,
                    "protocol": PROTOCOL_VERSION,
                    "capabilities": {
                        "ffmpeg": caps.ffmpeg.is_some(),
                        "ffprobe": caps.ffprobe.is_some(),
                        "sips": caps.sips.is_some(),
                        "pdfium": caps.pdfium.is_some(),
                        "vision": caps.vision.is_some(),
                    }
                }}),
            ),
            "inspect" => {
                let p = params!(InspectParams);
                let detected: Vec<Option<format::Format>> =
                    p.paths.iter().map(|f| format::detect(f).ok()).collect();
                let files: Vec<Value> = p
                    .paths
                    .iter()
                    .zip(&detected)
                    .map(|(path, f)| match f {
                        Some(f) => json!({ "path": path, "format": f, "label": f.label(), "category": f.category() }),
                        None => json!({ "path": path, "format": null }),
                    })
                    .collect();
                let (recipes, recipe_errors) = all_recipes(p.recipes_dir.as_ref());
                let wheel = catalog::wheel_for(&p.paths, &detected, caps, &recipes);
                send(
                    &out,
                    &json!({ "id": id, "result": { "files": files, "wheel": wheel, "recipe_errors": recipe_errors } }),
                );
            }
            "recipes" => {
                let p = params!(RecipesParams);
                let (_, errors) = all_recipes(p.recipes_dir.as_ref());
                // Built-ins first, then the user's, each with its file.
                let mut list: Vec<Value> = Recipe::builtin()
                    .into_iter()
                    .map(|r| {
                        let mut v = json!(r);
                        v["builtin"] = json!(true);
                        v
                    })
                    .collect();
                if let Some(d) = p.recipes_dir.clone().or_else(crate::default_recipes_dir) {
                    for (path, r) in Recipe::load_dir_with_paths(&d).0 {
                        let mut v = json!(r);
                        v["path"] = json!(path);
                        list.push(v);
                    }
                }
                send(
                    &out,
                    &json!({ "id": id, "result": { "recipes": list, "errors": errors } }),
                );
            }
            "privacy" => {
                // Media files are read with FFprobe, which takes a moment:
                // answer from a thread so cancels and runs aren't held up.
                let p = params!(PathsParams);
                let out = out.clone();
                threads.push(std::thread::spawn(move || {
                    let reports: Vec<Value> = p
                        .paths
                        .iter()
                        .map(|path| match veer_core::privacy::check(path, caps) {
                            Ok(r) => json!({ "path": path, "format": r.format, "findings": r.findings }),
                            Err(e) => json!({ "path": path, "error": { "code": e.code(), "message": e.to_string() } }),
                        })
                        .collect();
                    send(&out, &json!({ "id": id, "result": { "reports": reports } }));
                }));
            }
            "save_recipe" => {
                let p = params!(SaveRecipeParams);
                let dir = p.recipes_dir.clone().or_else(crate::default_recipes_dir);
                let result = match dir {
                    Some(d) => p.recipe.save(&d, p.replace.as_deref()),
                    None => Err(veer_core::Error::Invalid(
                        "there's no recipes folder".into(),
                    )),
                };
                match result {
                    Ok(path) => send(&out, &json!({ "id": id, "result": { "path": path } })),
                    Err(e) => send(&out, &error_response(&id, e.code(), e.to_string())),
                }
            }
            "run" => {
                let p = params!(RunParams);
                if p.paths.is_empty() {
                    send(&out, &error_response(&id, "bad_params", "no files given"));
                    continue;
                }
                let job_id = next_job.fetch_add(1, Ordering::Relaxed);
                let units = job::plan(&p.paths, &p.action, &p.options.output, &job::new_job_tag());
                send(
                    &out,
                    &json!({ "id": id, "result": { "job": job_id, "units": units.len() } }),
                );
                let cancel = Arc::new(AtomicBool::new(false));
                jobs.lock()
                    .expect("jobs lock")
                    .insert(job_id, cancel.clone());
                let executor = match (&worker, p.isolate) {
                    (Some((program, args)), true) => Executor::Subprocess {
                        program: program.clone(),
                        args: args.clone(),
                    },
                    _ => Executor::Inline,
                };
                let out = out.clone();
                let jobs = jobs.clone();
                threads.push(std::thread::spawn(move || {
                    let emit = |ev: job::Event| {
                        let mut v = serde_json::to_value(&ev).expect("events serialize");
                        v["job"] = json!(job_id);
                        send(&out, &v);
                    };
                    // job_finished must always be sent, even if something panics.
                    let summary = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        job::run_batch(units, &executor, &p.options, caps, &cancel, &emit)
                    }))
                    .unwrap_or_default();
                    jobs.lock().expect("jobs lock").remove(&job_id);
                    send(&out, &json!({ "job": job_id, "event": "job_finished", "summary": summary, "cancelled": cancel.load(Ordering::Relaxed) }));
                }));
            }
            "cancel" => {
                let p = params!(CancelParams);
                let found = jobs
                    .lock()
                    .expect("jobs lock")
                    .get(&p.job)
                    .map(|c| c.store(true, Ordering::Relaxed))
                    .is_some();
                send(&out, &json!({ "id": id, "result": { "cancelled": found } }));
            }
            "shutdown" => {
                send(&out, &json!({ "id": id, "result": {} }));
                break;
            }
            other => send(
                &out,
                &error_response(&id, "unknown_method", format!("no method \"{other}\"")),
            ),
        }
    }

    // Input closed or shutdown requested: stop everything still running.
    for cancel in jobs.lock().expect("jobs lock").values() {
        cancel.store(true, Ordering::Relaxed);
    }
    for t in threads {
        let _ = t.join();
    }
    0
}
