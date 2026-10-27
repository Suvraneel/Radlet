//! `radlet mcp`: a Model Context Protocol server on stdin and stdout, so AI
//! agents (Claude, and any other MCP client) can convert, compress, clean
//! and inspect files with Radlet, on this computer.
//!
//! It runs the same engine as the app and the CLI, with the same
//! guarantees: originals are never changed, results never overwrite a
//! file, and each conversion runs in its own worker process (inspecting
//! and the privacy check read files in this one). Nothing is uploaded. Password tools are left out, so passwords never pass through
//! an agent's conversation.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use radlet_core::job::{self, Event, Executor, Outcome, RunOptions, Status};
use radlet_core::ops::{CompressLevel, SplitMode, Step};
use radlet_core::{catalog, format, privacy, Action, Capabilities, Format, OutputLocation};
use serde_json::{json, Value};

/// Protocol revisions this server speaks, newest first.
const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Tool names from when loadouts were called recipes. They still work,
/// for agents set up back then, but aren't listed.
const OLD_NAMES: &[&str] = &["list_recipes", "run_recipe"];

const INSTRUCTIONS: &str = "Radlet converts and edits files on this computer: images, PDFs, audio, video, \
text and subtitles. Results are saved next to the originals (or in output_dir), with a new name: \
originals are never changed and nothing is overwritten. Call inspect_files first to see what a file \
can become. Paths should be absolute.";

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(out: &Out, v: &Value) {
    let mut o = out.lock().expect("stdout lock");
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn reply(out: &Out, id: Value, result: Value) {
    send(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    );
}

fn fail(out: &Out, id: Value, code: i64, message: &str) {
    send(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    );
}

/// Everything a tool call needs from the server.
#[derive(Clone)]
struct Context {
    caps: &'static Capabilities,
    worker: Option<(PathBuf, Vec<String>)>,
    recipes_dir: Option<PathBuf>,
}

pub fn serve(
    input: impl BufRead,
    output: Box<dyn Write + Send>,
    caps: &'static Capabilities,
    worker: Option<(PathBuf, Vec<String>)>,
    recipes_dir: Option<PathBuf>,
) -> i32 {
    let out: Out = Arc::new(Mutex::new(output));
    let ctx = Context {
        caps,
        worker,
        recipes_dir,
    };
    // Cancel flags of calls in progress, by request id.
    let running: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> = Arc::default();
    let mut threads: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        threads.retain(|t| !t.is_finished());
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                fail(&out, Value::Null, -32700, &format!("parse error: {e}"));
                continue;
            }
        };
        if msg.is_array() {
            fail(
                &out,
                Value::Null,
                -32600,
                "batches aren't supported; send one message per line",
            );
            continue;
        }
        // Responses to anything we sent are ignored; we send no requests.
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = msg.get("id").cloned() else {
            if method == "notifications/cancelled" {
                if let Some(rid) = params.get("requestId") {
                    if let Some(flag) = running.lock().expect("lock").get(&rid.to_string()) {
                        flag.store(true, Ordering::Relaxed);
                    }
                }
            }
            continue;
        };
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked
                    .filter(|v| VERSIONS.contains(v))
                    .unwrap_or(VERSIONS[0]);
                reply(
                    &out,
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "radlet", "title": "Radlet", "version": radlet_core::VERSION },
                        "instructions": INSTRUCTIONS,
                    }),
                );
            }
            "ping" => reply(&out, id, json!({})),
            "tools/list" => reply(&out, id, json!({ "tools": tools(ctx.caps) })),
            "tools/call" => {
                let Some(name) = params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                else {
                    fail(&out, id, -32602, "tools/call needs a tool name");
                    continue;
                };
                let old_name = OLD_NAMES.contains(&name.as_str());
                if !old_name && !tools(ctx.caps).iter().any(|t| t["name"] == name.as_str()) {
                    fail(&out, id, -32602, &format!("there's no tool called {name}"));
                    continue;
                }
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let token = params
                    .get("_meta")
                    .and_then(|m| m.get("progressToken"))
                    .cloned();
                let cancel = Arc::new(AtomicBool::new(false));
                {
                    let mut calls = running.lock().expect("lock");
                    if calls.contains_key(&id.to_string()) {
                        drop(calls);
                        fail(&out, id, -32600, "a request with this id is still running");
                        continue;
                    }
                    calls.insert(id.to_string(), cancel.clone());
                }
                let (out, ctx, running) = (out.clone(), ctx.clone(), running.clone());
                threads.push(std::thread::spawn(move || {
                    let progress_out = out.clone();
                    let mut last = -1.0f32;
                    let mut progress = |p: f32| {
                        let Some(token) = &token else { return };
                        // Progress must only go up.
                        if p <= last || (p - last < 0.02 && p < 1.0) {
                            return;
                        }
                        last = p;
                        send(
                            &progress_out,
                            &json!({ "jsonrpc": "2.0", "method": "notifications/progress",
                                     "params": { "progressToken": token, "progress": p, "total": 1.0 } }),
                        );
                    };
                    // A surprise inside a tool must still answer the request.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        call(&name, &args, &ctx, &cancel, &mut progress)
                    }))
                    .unwrap_or_else(|_| Err("Radlet hit a problem running this tool".to_string()));
                    let result = match outcome {
                        Ok((text, structured)) => json!({
                            "content": [{ "type": "text", "text": text }],
                            "structuredContent": structured,
                            "isError": false,
                        }),
                        Err(message) => json!({
                            "content": [{ "type": "text", "text": message }],
                            "isError": true,
                        }),
                    };
                    running.lock().expect("lock").remove(&id.to_string());
                    // A cancelled request gets no response.
                    if !cancel.load(Ordering::Relaxed) {
                        reply(&out, id, result);
                    }
                }));
            }
            _ => fail(&out, id, -32601, &format!("unknown method {method}")),
        }
    }
    // The client went away: stop running work, then wait for it.
    for flag in running.lock().expect("lock").values() {
        flag.store(true, Ordering::Relaxed);
    }
    for t in threads {
        let _ = t.join();
    }
    0
}

// ---------------------------------------------------------------------------
// Tool definitions

fn paths_schema() -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "minItems": 1,
            "description": "Absolute paths of the files." })
}

fn output_dir_schema() -> Value {
    json!({ "type": "string",
            "description": "Folder for the results. Default: next to each original." })
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required,
                         "additionalProperties": false },
        "annotations": { "title": title, "readOnlyHint": read_only, "destructiveHint": false,
                         "idempotentHint": read_only, "openWorldHint": false },
    })
}

const STEPS_HELP: &str = "Each step is an object with \"op\": \
convert {to, quality?}; resize {max | percent | width | height}; rotate {degrees: 90|180|270}; \
crop {rect: {x, y, w, h} as fractions}; redact {rects, style?: black|pixelate|blur}; strip_metadata; \
compress {level?: light|balanced|strong|smart, target_bytes?}; pages {pages: [numbers], rotate?: [{page, degrees}]}; \
page_numbers {position?, style?: number|page|page_of_total|fraction, start?, skip_first?}; \
watermark {text, opacity?, diagonal?}; trim {start?, end?} in seconds; normalize_audio; \
remove_background {crop?}; make_searchable; transcribe {to?: srt|vtt|txt, language?}; \
redact_text {words?: [phrases], emails?, phones?, cards?} blacks out text in PDFs for real (Mac). \
Steps run in order on each file, for example [{\"op\":\"resize\",\"max\":1600},{\"op\":\"convert\",\"to\":\"webp\"}].";

fn tools(caps: &Capabilities) -> Vec<Value> {
    let mut list = vec![
        tool(
            "inspect_files",
            "Inspect files",
            "What each file is, the formats it can be converted to, and which steps apply to it.",
            json!({ "paths": paths_schema() }),
            &["paths"],
            true,
        ),
        tool(
            "convert",
            "Convert",
            "Convert files to another format, such as HEIC to JPEG, SVG to PNG, MKV to MP4, \
             WAV to MP3, Markdown to Word or PDF, Word to Markdown, CSV to JSON, images to PDF, \
             a PDF to one image per page, a folder to a ZIP, or a ZIP to a folder.",
            json!({
                "paths": paths_schema(),
                "to": { "type": "string", "description": "Target format, such as jpeg, png, webp, pdf, mp4, m4a, mp3, txt, md, html, docx, rtf, csv, json, yaml, toml, xml, srt, vtt, ass, zip, tar or folder." },
                "quality": { "type": "integer", "minimum": 1, "maximum": 100, "description": "For lossy formats." },
                "output_dir": output_dir_schema(),
            }),
            &["paths", "to"],
            false,
        ),
        tool(
            "compress",
            "Compress",
            "Make images, PDFs and videos smaller, by level or to fit a size (such as 2MB). \
             The smart level finds the smallest JPEG or WebP that still looks the same.",
            json!({
                "paths": paths_schema(),
                "level": { "type": "string", "enum": ["light", "balanced", "strong", "smart"] },
                "target_size": { "type": "string", "description": "Largest size, such as 500KB or 2MB." },
                "output_dir": output_dir_schema(),
            }),
            &["paths"],
            false,
        ),
        tool(
            "run_steps",
            "Run steps",
            &format!("Run a chain of editing steps on each file. {STEPS_HELP}"),
            json!({
                "paths": paths_schema(),
                "steps": { "type": "array", "items": { "type": "object" }, "minItems": 1 },
                "output_dir": output_dir_schema(),
            }),
            &["paths", "steps"],
            false,
        ),
        tool(
            "combine_into_pdf",
            "Combine into PDF",
            "Put images, PDFs, SVGs and documents, in the order given, into one PDF.",
            json!({ "paths": paths_schema(), "output_dir": output_dir_schema() }),
            &["paths"],
            false,
        ),
        tool(
            "split_pdf",
            "Split PDF",
            "Split PDFs into one file per page, per group of pages, or per range.",
            json!({
                "paths": paths_schema(),
                "ranges": { "type": "string", "description": "Such as \"1-3, 5, 8-\". Default: every page." },
                "every": { "type": "integer", "minimum": 1, "description": "Pages per part." },
                "output_dir": output_dir_schema(),
            }),
            &["paths"],
            false,
        ),
        tool(
            "extract_text",
            "Extract text",
            "The text of documents (TXT, Markdown, HTML, Word, RTF, CSV, JSON) and PDFs, and (on a Mac) \
             the text recognised in photos and scans. Writes nothing.",
            json!({ "paths": paths_schema() }),
            &["paths"],
            true,
        ),
        tool(
            "count_words",
            "Count words",
            "Words and reading time of each document or PDF. Writes nothing.",
            json!({ "paths": paths_schema() }),
            &["paths"],
            true,
        ),
        tool(
            "zip",
            "Zip",
            "Put files and folders, together, into one ZIP archive.",
            json!({ "paths": paths_schema(), "output_dir": output_dir_schema() }),
            &["paths"],
            false,
        ),
        tool(
            "privacy_check",
            "Privacy check",
            "What files reveal before they're shared: location, camera or phone, names, dates, \
             editing apps, and hidden content such as previews, earlier PDF versions and attachments. \
             Each finding says whether strip_metadata removes it.",
            json!({ "paths": paths_schema() }),
            &["paths"],
            true,
        ),
        tool(
            "checksum",
            "Checksum",
            "SHA-256 of each file. Writes nothing.",
            json!({ "paths": paths_schema() }),
            &["paths"],
            true,
        ),
        tool(
            "list_loadouts",
            "List loadouts",
            "The user's saved loadouts (named chains of steps) and the built-in ones.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "run_loadout",
            "Run loadout",
            "Run a saved loadout, by name, on each file.",
            json!({
                "paths": paths_schema(),
                "loadout": { "type": "string" },
                "output_dir": output_dir_schema(),
            }),
            &["paths", "loadout"],
            false,
        ),
    ];
    if caps.media() || caps.native_media() {
        list.push(tool(
            "join_clips",
            "Join clips",
            "Put videos, in the order given, one after another into one MP4 (each fitted into the \
             first one's shape), or sound files into one M4A.",
            json!({ "paths": paths_schema(), "output_dir": output_dir_schema() }),
            &["paths"],
            false,
        ));
        list.push(tool(
            "burn_subtitles",
            "Burn in subtitles",
            "Draw subtitles into videos' pictures so every player shows them. Give each video \
             with the subtitle file (SRT, VTT, ASS, SBV, TTML) named after it, or one of each.",
            json!({ "paths": paths_schema(), "output_dir": output_dir_schema() }),
            &["paths"],
            false,
        ));
    }
    if caps.vision.is_some() {
        list.push(tool(
            "scan_documents",
            "Scan documents",
            "Make photos of paper pages, in the order given, into one scanned PDF: each page cut \
             out, squared up and evened out, with searchable text.",
            json!({ "paths": paths_schema(), "output_dir": output_dir_schema() }),
            &["paths"],
            false,
        ));
    }
    if caps.media() {
        list.push(tool(
            "extract_frames",
            "Save stills",
            "Save evenly spaced still images from videos.",
            json!({
                "paths": paths_schema(),
                "count": { "type": "integer", "minimum": 1, "maximum": 500 },
                "format": { "type": "string", "enum": ["jpeg", "png"] },
                "output_dir": output_dir_schema(),
            }),
            &["paths"],
            false,
        ));
    }
    if caps.vision.is_some() && caps.media() {
        list.push(tool(
            "make_subtitles",
            "Make subtitles",
            "Subtitles (SRT or WebVTT) or a transcript (txt) of the speech in audio and video, \
             by on-device speech recognition.",
            json!({
                "paths": paths_schema(),
                "format": { "type": "string", "enum": ["srt", "vtt", "txt"] },
                "language": { "type": "string", "description": "Such as en-US. Default: the Mac's language." },
                "output_dir": output_dir_schema(),
            }),
            &["paths"],
            false,
        ));
    }
    list
}

// ---------------------------------------------------------------------------
// Calls

fn paths(args: &Value) -> Result<Vec<PathBuf>, String> {
    let list = args
        .get("paths")
        .and_then(Value::as_array)
        .ok_or("paths must be a list of file paths")?;
    if list.is_empty() {
        return Err("give at least one path".into());
    }
    list.iter()
        .map(|p| {
            let s = p.as_str().ok_or("each path must be a string")?;
            let path = PathBuf::from(s);
            if !path.exists() {
                return Err(format!("{s} doesn't exist"));
            }
            Ok(path)
        })
        .collect()
}

fn output_dir(args: &Value) -> Result<Option<PathBuf>, String> {
    match args.get("output_dir").and_then(Value::as_str) {
        None | Some("") => Ok(None),
        Some(d) => {
            let p = PathBuf::from(d);
            if p.is_dir() {
                Ok(Some(p))
            } else {
                Err(format!("{d} isn't a folder"))
            }
        }
    }
}

fn format_arg(args: &Value, key: &str) -> Result<Option<Format>, String> {
    match args.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => serde_json::from_value(json!(s.trim_start_matches('.').to_lowercase()))
            .map(Some)
            .map_err(|_| format!("\"{s}\" isn't a format Radlet knows")),
    }
}

/// Runs a tool: a summary for people and the same as JSON, or why not.
fn call(
    name: &str,
    args: &Value,
    ctx: &Context,
    cancel: &AtomicBool,
    progress: &mut (dyn FnMut(f32) + Send),
) -> Result<(String, Value), String> {
    match name {
        "inspect_files" => inspect(&paths(args)?, ctx.caps),
        "privacy_check" => privacy_check(&paths(args)?, ctx.caps),
        // The old names (from when loadouts were recipes) still work.
        "list_loadouts" | "list_recipes" => {
            let all = crate::recipes(ctx.recipes_dir.as_ref());
            let text = all
                .iter()
                .map(|r| format!("{}: {}", r.name, r.description.clone().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\n");
            // Answer in the shape the caller asked for.
            let key = if name == "list_recipes" {
                "recipes"
            } else {
                "loadouts"
            };
            Ok((text, json!({ key: all })))
        }
        "convert" => {
            let to = format_arg(args, "to")?.ok_or("say which format to convert to")?;
            let quality = match args.get("quality") {
                None | Some(Value::Null) => None,
                Some(q) => Some(
                    q.as_u64()
                        .filter(|q| (1..=100).contains(q))
                        .ok_or("quality is a number from 1 to 100")? as u8,
                ),
            };
            run(
                args,
                Action::single(Step::Convert { to, quality }),
                ctx,
                cancel,
                progress,
            )
        }
        "compress" => {
            let level = match args.get("level").and_then(Value::as_str) {
                None => CompressLevel::Balanced,
                Some(l) => serde_json::from_value(json!(l))
                    .map_err(|_| "level is light, balanced, strong or smart".to_string())?,
            };
            let target_bytes = match args.get("target_size").and_then(Value::as_str) {
                None => None,
                Some(s) => Some(
                    radlet_core::ops::parse_size(s)
                        .ok_or_else(|| format!("\"{s}\" isn't a size (try 500KB or 2MB)"))?,
                ),
            };
            run(
                args,
                Action::single(Step::Compress {
                    level,
                    target_bytes,
                }),
                ctx,
                cancel,
                progress,
            )
        }
        "run_steps" => {
            let steps: Vec<Step> =
                serde_json::from_value(args.get("steps").cloned().unwrap_or(Value::Null))
                    .map_err(|e| format!("the steps aren't right: {e}. {STEPS_HELP}"))?;
            if steps.is_empty() {
                return Err("give at least one step".into());
            }
            run(
                args,
                Action::Steps { steps, label: None },
                ctx,
                cancel,
                progress,
            )
        }
        "combine_into_pdf" => run(args, Action::CombinePdf, ctx, cancel, progress),
        "join_clips" => run(args, Action::JoinMedia, ctx, cancel, progress),
        "burn_subtitles" => run(args, Action::BurnSubtitles, ctx, cancel, progress),
        "scan_documents" => run(args, Action::ScanDocuments, ctx, cancel, progress),
        "split_pdf" => {
            let mode = match (
                args.get("ranges").and_then(Value::as_str),
                args.get("every").and_then(Value::as_u64),
            ) {
                (Some(r), _) => SplitMode::Ranges { ranges: r.into() },
                (None, Some(n)) => SplitMode::Chunks {
                    size: n.min(u32::MAX as u64) as u32,
                },
                (None, None) => SplitMode::EveryPage,
            };
            run(args, Action::SplitPdf { mode }, ctx, cancel, progress)
        }
        "extract_text" => run(args, Action::CopyText, ctx, cancel, progress),
        "count_words" => run(args, Action::WordCount, ctx, cancel, progress),
        "zip" => run(args, Action::ZipAll, ctx, cancel, progress),
        "checksum" => run(args, Action::Checksum, ctx, cancel, progress),
        "run_loadout" | "run_recipe" => {
            let wanted = args
                .get("loadout")
                .or_else(|| args.get("recipe"))
                .and_then(Value::as_str)
                .ok_or("say which loadout to run")?;
            let all = crate::recipes(ctx.recipes_dir.as_ref());
            let recipe = radlet_core::Recipe::find(wanted, &all).map_err(|e| e.to_string())?;
            run(args, recipe.action(), ctx, cancel, progress)
        }
        "extract_frames" => {
            let count = args
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .min(500) as u32;
            let format = format_arg(args, "format")?;
            run(
                args,
                Action::ExtractFrames { count, format },
                ctx,
                cancel,
                progress,
            )
        }
        "make_subtitles" => {
            let to = format_arg(args, "format")?.unwrap_or(Format::Srt);
            let language = args
                .get("language")
                .and_then(Value::as_str)
                .map(str::to_string);
            run(
                args,
                Action::single(Step::Transcribe { to, language }),
                ctx,
                cancel,
                progress,
            )
        }
        other => Err(format!("there's no tool called {other}")),
    }
}

fn run(
    args: &Value,
    action: Action,
    ctx: &Context,
    cancel: &AtomicBool,
    progress: &mut (dyn FnMut(f32) + Send),
) -> Result<(String, Value), String> {
    let files = paths(args)?;
    let options = RunOptions {
        output: output_dir(args)?
            .map(OutputLocation::Directory)
            .unwrap_or_default(),
        parallelism: None,
        timeout_secs: None,
    };
    let executor = match &ctx.worker {
        Some((program, args)) => Executor::Subprocess {
            program: program.clone(),
            args: args.clone(),
        },
        None => Executor::Inline,
    };
    let units = job::plan(&files, &action, &options.output, &job::new_job_tag());
    let total = units.len().max(1) as f32;
    let outcomes: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    let shares: Mutex<HashMap<usize, f32>> = Mutex::new(HashMap::new());
    let progress = Mutex::new(progress);
    let on_event = |ev: Event| match ev {
        Event::ItemProgress { index, fraction } => {
            let mut s = shares.lock().expect("lock");
            s.insert(index, fraction.clamp(0.0, 1.0));
            let p = s.values().sum::<f32>() / total;
            let mut report = progress.lock().expect("lock");
            (*report)(p.min(1.0));
        }
        Event::ItemFinished { outcome } => {
            shares.lock().expect("lock").insert(outcome.index, 1.0);
            outcomes.lock().expect("lock").push(outcome);
        }
        Event::ItemStarted { .. } => {}
    };
    let summary = job::run_batch(units, &executor, &options, ctx.caps, cancel, &on_event);
    let mut outcomes = outcomes.into_inner().expect("lock");
    outcomes.sort_by_key(|o| o.index);
    let lines: Vec<String> = outcomes.iter().map(describe).collect();
    let structured = json!({ "summary": summary, "results": outcomes });
    let text = lines.join("\n");
    if summary.done == 0 && summary.failed > 0 {
        return Err(text);
    }
    Ok((text, structured))
}

fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn describe(o: &Outcome) -> String {
    let inputs = o
        .inputs
        .iter()
        .map(|p| name(p))
        .collect::<Vec<_>>()
        .join(", ");
    match o.status {
        Status::Done => match (&o.value, o.outputs.as_slice()) {
            (Some(v), _) => format!("{inputs}:\n{v}"),
            (None, []) => format!("✓ {inputs}"),
            (None, outs) => format!(
                "✓ {inputs} → {}",
                outs.iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        _ => format!(
            "✗ {inputs}: {}",
            o.error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| format!("{:?}", o.status).to_lowercase())
        ),
    }
}

fn inspect(files: &[PathBuf], caps: &Capabilities) -> Result<(String, Value), String> {
    let detected: Vec<Option<Format>> = files.iter().map(|f| format::detect(f).ok()).collect();
    let targets = catalog::selection_targets(&detected, caps);
    let candidates = [
        ("resize", Step::resize_max(1000)),
        ("rotate", Step::Rotate { degrees: 90 }),
        (
            "crop",
            Step::Crop {
                rect: radlet_core::ops::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 0.5,
                },
            },
        ),
        ("strip_metadata", Step::StripMetadata),
        ("compress", Step::compress(CompressLevel::Balanced)),
        (
            "pages",
            Step::Pages {
                pages: vec![1],
                rotate: vec![],
            },
        ),
        (
            "page_numbers",
            Step::PageNumbers {
                position: Default::default(),
                style: Default::default(),
                start: 1,
                size: None,
                skip_first: false,
            },
        ),
        (
            "watermark",
            Step::Watermark {
                text: "x".into(),
                opacity: 0.15,
                size: None,
                diagonal: true,
            },
        ),
        (
            "trim",
            Step::Trim {
                start: 0.0,
                end: None,
            },
        ),
        ("normalize_audio", Step::NormalizeAudio),
        ("remove_background", Step::RemoveBackground { crop: false }),
        ("make_searchable", Step::MakeSearchable),
        (
            "transcribe",
            Step::Transcribe {
                to: Format::Srt,
                language: None,
            },
        ),
    ];
    let known: Vec<Format> = detected.iter().flatten().copied().collect();
    let steps: Vec<&str> = if known.len() == files.len() {
        candidates
            .iter()
            .filter(|(_, s)| known.iter().all(|f| catalog::step_accepts(s, *f, caps)))
            .map(|(n, _)| *n)
            .collect()
    } else {
        Vec::new()
    };
    let described: Vec<Value> = files
        .iter()
        .zip(&detected)
        .map(|(p, f)| match f {
            Some(f) => {
                json!({ "path": p, "format": f, "label": f.label(), "category": f.category() })
            }
            None => json!({ "path": p, "format": null }),
        })
        .collect();
    let mut text = String::new();
    for (p, f) in files.iter().zip(&detected) {
        text.push_str(&format!(
            "{}: {}\n",
            name(p),
            f.map(|f| f.label().to_string())
                .unwrap_or_else(|| "not a format Radlet knows".into())
        ));
    }
    let target_names: Vec<String> = targets
        .iter()
        .map(|t| {
            serde_json::to_value(t)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default()
        })
        .collect();
    text.push_str(&format!("Convert to: {}\n", target_names.join(", ")));
    text.push_str(&format!("Steps that apply: {}", steps.join(", ")));
    Ok((
        text,
        json!({ "files": described, "convert_to": target_names, "steps": steps }),
    ))
}

fn privacy_check(files: &[PathBuf], caps: &Capabilities) -> Result<(String, Value), String> {
    let mut text = String::new();
    let mut reports = Vec::new();
    for p in files {
        text.push_str(&format!("{}\n", name(p)));
        match privacy::check(p, caps) {
            Ok(r) => {
                if r.findings.is_empty() {
                    text.push_str("  Nothing found.\n");
                }
                for f in &r.findings {
                    let stays = if f.removable {
                        ""
                    } else {
                        " (strip_metadata leaves this)"
                    };
                    text.push_str(&format!("  {}: {}{stays}\n", f.label, f.detail));
                }
                reports.push(json!({ "path": p, "format": r.format, "findings": r.findings }));
            }
            Err(e) => {
                text.push_str(&format!("  ✗ {e}\n"));
                reports.push(json!({ "path": p, "error": e.to_string() }));
            }
        }
    }
    Ok((text.trim_end().to_string(), json!({ "reports": reports })))
}
