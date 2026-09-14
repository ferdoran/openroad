//! BRP perf probe: a JSON-RPC client for the client's Bevy Remote Protocol
//! server (enabled by `dev_tools: true` in config.yaml, HTTP on port 15702).
//! MCP-based BRP tooling doesn't work in this environment, so perf sessions
//! need a shell-composable way to read the `openroad/diagnostics` dump (FPS,
//! `world_counts/*`, `cache_counts/*`, render-pass times) and to flip
//! `RenderDebugSettings` fields remotely. `attribute` automates the
//! toggle-off → settle → sample → restore loop into a per-subsystem
//! frame-cost table; `sample` records the dump as JSONL for offline diffing.
//!
//! Measurement note: `avg` is the mean of the diagnostic's ~120-measurement
//! history, i.e. ~2 s of frames at 60 fps but ~8 s at 15 fps — pick settle
//! times of at least `120 / expected_fps` seconds or the average still
//! contains pre-toggle frames.

use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{App, AppSettings, Arg, SubCommand};
use serde_json::{json, Map, Value};

/// Full type path required by `world.get_resources`/`world.mutate_resources`
/// (BRP resolves resources via `TypeRegistry::get_with_type_path`, no
/// short-name fallback).
const SETTINGS_TYPE_PATH: &str = "client::plugins::dev::render_debug::RenderDebugSettings";

/// Subsystem toggles `attribute` sweeps. `enable_shadows` is deliberately
/// absent: it defaults to off, so toggling it off measures nothing.
const ATTRIBUTE_FIELDS: [&str; 9] = [
    "render_terrain",
    "render_objects",
    "render_water",
    "render_foliage",
    "render_effects",
    "play_animations",
    "enable_fog",
    "backface_culling",
    "automatic_batching",
];

struct Brp {
    url: String,
    agent: ureq::Agent,
}

impl Brp {
    fn new(host: &str, port: u16) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .build();
        Self {
            url: format!("http://{host}:{port}/"),
            agent,
        }
    }

    fn call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let mut request = json!({ "jsonrpc": "2.0", "id": 1, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        let response = self
            .agent
            .post(&self.url)
            .send_json(request)
            .map_err(|e| match e {
                // Three distinct causes reach this one transport error, and the
                // message used to name only the first — and by its old name.
                ureq::Error::Transport(t) => format!(
                    "BRP not reachable at {url} ({t})\n\
                 \n\
                 Check, in order:\n\
                 - `diagnostics: true` in the config.yaml the client actually loaded.\n\
                 \x20  That path is relative to the client's working directory, not to\n\
                 \x20  this repo: `make run wsl` starts the exe from the WSL checkout,\n\
                 \x20  so it reads that copy. The flag is restart-only.\n\
                 - the client is past loading (GameState::Game).\n\
                 - you are on the same machine as the client. The server binds\n\
                 \x20  127.0.0.1, and WSL's loopback is not Windows's — run the\n\
                 \x20  Windows brp_perf.exe against a Windows client\n\
                 \x20  (docs/perf-remote.md).",
                    url = self.url
                ),
                other => format!("BRP request failed: {other}"),
            })?;
        let body: Value = response
            .into_json()
            .map_err(|e| format!("invalid BRP response: {e}"))?;
        if let Some(error) = body.get("error") {
            return Err(format!("BRP error from {method}: {error}"));
        }
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    }

    /// The `openroad/diagnostics` dump: `{path: {value, avg, smoothed}}`.
    fn diagnostics(&self) -> Result<Map<String, Value>, String> {
        match self.call("openroad/diagnostics", None)? {
            Value::Object(map) => Ok(map),
            other => Err(format!("unexpected diagnostics payload: {other}")),
        }
    }

    fn get_settings(&self) -> Result<Value, String> {
        let result = self.call(
            "world.get_resources",
            Some(json!({ "resource": SETTINGS_TYPE_PATH })),
        )?;
        Ok(result.get("value").cloned().unwrap_or(result))
    }

    fn set_setting(&self, field: &str, value: &Value) -> Result<(), String> {
        self.call(
            "world.mutate_resources",
            Some(json!({ "resource": SETTINGS_TYPE_PATH, "path": field, "value": value })),
        )
        .map(|_| ())
    }
}

/// Settle, then read the ~120-frame `avg` of fps and frame_time (ms) once —
/// the store's history window does the averaging, no polling loop needed.
fn measure(brp: &Brp, settle_secs: f64) -> Result<(f64, f64), String> {
    std::thread::sleep(Duration::from_secs_f64(settle_secs));
    let diag = brp.diagnostics()?;
    let avg = |key: &str| {
        diag.get(key)
            .and_then(|d| d.get("avg"))
            .and_then(Value::as_f64)
    };
    match (avg("fps"), avg("frame_time")) {
        (Some(fps), Some(frame_time)) => Ok((fps, frame_time)),
        _ => Err(
            "diagnostics dump has no fps/frame_time entries yet — is the game past loading?".into(),
        ),
    }
}

fn cmd_snapshot(brp: &Brp, prefix: Option<&str>, raw_json: bool) -> Result<(), String> {
    let diag = brp.diagnostics()?;
    let filtered: Map<String, Value> = diag
        .into_iter()
        .filter(|(key, _)| prefix.is_none_or(|p| key.starts_with(p)))
        .collect();
    if raw_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(filtered)).unwrap()
        );
        return Ok(());
    }
    println!(
        "{:<44} {:>12} {:>12} {:>12}",
        "diagnostic", "value", "avg", "smoothed"
    );
    // serde_json::Map is a BTreeMap, so iteration is already key-sorted.
    for (key, entry) in &filtered {
        let field = |name: &str| match entry.get(name).and_then(Value::as_f64) {
            Some(v) => format!("{v:.2}"),
            None => "-".to_string(),
        };
        println!(
            "{key:<44} {:>12} {:>12} {:>12}",
            field("value"),
            field("avg"),
            field("smoothed")
        );
    }
    Ok(())
}

fn cmd_sample(brp: &Brp, secs: f64, interval_ms: u64, out: &str) -> Result<(), String> {
    let mut file = File::create(out).map_err(|e| format!("cannot create {out}: {e}"))?;
    let start = Instant::now();
    let mut samples = 0usize;
    while start.elapsed().as_secs_f64() < secs {
        let diag = brp.diagnostics()?;
        let t_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut line = Map::new();
        line.insert("t_ms".to_string(), json!(t_ms));
        line.extend(diag);
        writeln!(file, "{}", Value::Object(line)).map_err(|e| format!("write failed: {e}"))?;
        samples += 1;
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
    println!("wrote {samples} samples to {out}");
    Ok(())
}

fn cmd_fps(brp: &Brp, settle_secs: f64) -> Result<(), String> {
    let (fps, frame_time) = measure(brp, settle_secs)?;
    println!("fps: {fps:.2}  frame_time: {frame_time:.3} ms  (avg over ~120 frames)");
    Ok(())
}

fn cmd_set(brp: &Brp, field: &str, value: &str) -> Result<(), String> {
    // Bools are the normal case; anything else is passed through as JSON so
    // future non-bool settings fields keep working.
    let value: Value =
        serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    brp.set_setting(field, &value)?;
    println!("{field} = {value}");
    Ok(())
}

/// Restore every swept field to its pre-sweep value; used both on success and
/// as the error cleanup path so an aborted run never leaves subsystems off.
fn restore_all(brp: &Brp, originals: &Map<String, Value>) -> Result<(), String> {
    let mut failures = Vec::new();
    for field in ATTRIBUTE_FIELDS {
        if let Some(original) = originals.get(field) {
            if brp.set_setting(field, original).is_err() {
                failures.push(field);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "failed to restore settings: {failures:?} — check the render-debug egui panel"
        ))
    }
}

fn cmd_attribute(brp: &Brp, secs: f64) -> Result<(), String> {
    let settings = brp.get_settings()?;
    let originals = settings
        .as_object()
        .ok_or_else(|| format!("unexpected settings payload: {settings}"))?
        .clone();

    eprintln!("measuring baseline ({secs:.0}s settle per step)...");
    let (fps_base, ft_base) = measure(brp, secs)?;
    eprintln!("baseline: {fps_base:.1} fps / {ft_base:.2} ms");

    let mut rows: Vec<(&str, Option<(f64, f64)>)> = Vec::new();
    let mut sweep_result: Result<(), String> = Ok(());
    for field in ATTRIBUTE_FIELDS {
        if originals.get(field) != Some(&Value::Bool(true)) {
            rows.push((field, None)); // already off (or missing): nothing to measure
            continue;
        }
        eprintln!("toggling {field} off...");
        sweep_result = brp
            .set_setting(field, &json!(false))
            .and_then(|_| measure(brp, secs))
            .and_then(|(fps_off, ft_off)| {
                rows.push((field, Some((fps_off, ft_off))));
                brp.set_setting(field, &json!(true))
            });
        if sweep_result.is_err() {
            break;
        }
    }
    let restore_result = restore_all(brp, &originals);
    sweep_result?;
    restore_result?;

    println!(
        "{:<22} {:>8} {:>8} {:>9}",
        "subsystem", "fps_on", "fps_off", "cost_ms"
    );
    for (field, sample) in &rows {
        match sample {
            Some((fps_off, ft_off)) => println!(
                "{field:<22} {fps_base:>8.1} {fps_off:>8.1} {:>9.3}",
                ft_base - ft_off
            ),
            None => println!("{field:<22} {:>8} {:>8} {:>9}", "-", "-", "(already off)"),
        }
    }

    let (fps_end, ft_end) = measure(brp, secs)?;
    println!("baseline recheck: {fps_end:.1} fps / {ft_end:.2} ms");
    if ft_base > f64::EPSILON && ((ft_end - ft_base) / ft_base).abs() > 0.10 {
        eprintln!(
            "warning: baseline drifted {:+.0}% during the sweep (streaming/scene load?) — \
             cost numbers are unreliable, re-run standing still in a fully loaded area",
            (ft_end - ft_base) / ft_base * 100.0
        );
    }
    Ok(())
}

fn main() {
    let matches = App::new("brp_perf")
        .version("0.1.0")
        .about("Query and drive the client's Bevy Remote Protocol server for perf insights")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .arg(
            Arg::with_name("host")
                .long("host")
                .takes_value(true)
                .default_value("127.0.0.1")
                .global(true)
                .help("BRP server host"),
        )
        .arg(
            Arg::with_name("port")
                .long("port")
                .takes_value(true)
                .global(true)
                .help("BRP server port (default: $BRP_PORT, $BRP_EXTRAS_PORT, or 15702)"),
        )
        .subcommand(
            SubCommand::with_name("snapshot")
                .about("One-shot dump of all diagnostics (openroad/diagnostics)")
                .arg(
                    Arg::with_name("prefix")
                        .long("prefix")
                        .takes_value(true)
                        .help("Only keys with this prefix, e.g. world_counts/"),
                )
                .arg(
                    Arg::with_name("json")
                        .long("json")
                        .help("Raw JSON instead of a table"),
                ),
        )
        .subcommand(
            SubCommand::with_name("sample")
                .about("Poll diagnostics into a JSONL file for offline diffing")
                .arg(
                    Arg::with_name("secs")
                        .long("secs")
                        .takes_value(true)
                        .default_value("30"),
                )
                .arg(
                    Arg::with_name("interval-ms")
                        .long("interval-ms")
                        .takes_value(true)
                        .default_value("250"),
                )
                .arg(
                    Arg::with_name("out")
                        .long("out")
                        .takes_value(true)
                        .help("Output path (default: perf-<unix>.jsonl)"),
                ),
        )
        .subcommand(
            SubCommand::with_name("fps")
                .about("Settle, then print the ~120-frame average fps / frame time")
                .arg(
                    Arg::with_name("settle-secs")
                        .long("settle-secs")
                        .takes_value(true)
                        .default_value("3"),
                ),
        )
        .subcommand(SubCommand::with_name("get").about("Print RenderDebugSettings"))
        .subcommand(
            SubCommand::with_name("set")
                .about("Set a RenderDebugSettings field (world.mutate_resources)")
                .arg(
                    Arg::with_name("field")
                        .required(true)
                        .help("e.g. render_effects"),
                )
                .arg(Arg::with_name("value").required(true).help("e.g. false")),
        )
        .subcommand(
            SubCommand::with_name("attribute")
                .about("Toggle each subsystem off/on and print a per-subsystem frame-cost table")
                .arg(
                    Arg::with_name("secs")
                        .long("secs")
                        .takes_value(true)
                        .default_value("3")
                        .help("Settle time per step; use >= 120/fps seconds"),
                ),
        )
        .get_matches();

    let host = matches.value_of("host").unwrap_or("127.0.0.1").to_string();
    let port: u16 = matches
        .value_of("port")
        .map(str::to_string)
        .or_else(|| std::env::var("BRP_PORT").ok())
        .or_else(|| std::env::var("BRP_EXTRAS_PORT").ok())
        .map(|p| {
            p.parse()
                .unwrap_or_else(|_| exit_with(&format!("invalid port: {p}")))
        })
        .unwrap_or(15702);
    let brp = Brp::new(&host, port);

    let parse_f64 = |m: &clap::ArgMatches, name: &str| -> f64 {
        let raw = m.value_of(name).unwrap();
        raw.parse()
            .unwrap_or_else(|_| exit_with(&format!("invalid --{name}: {raw}")))
    };

    let result = match matches.subcommand() {
        ("snapshot", Some(m)) => cmd_snapshot(&brp, m.value_of("prefix"), m.is_present("json")),
        ("sample", Some(m)) => {
            let default_out = format!(
                "perf-{}.jsonl",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            );
            let out = m.value_of("out").map(str::to_string).unwrap_or(default_out);
            let interval = m.value_of("interval-ms").unwrap();
            let interval: u64 = interval
                .parse()
                .unwrap_or_else(|_| exit_with(&format!("invalid --interval-ms: {interval}")));
            cmd_sample(&brp, parse_f64(m, "secs"), interval, &out)
        }
        ("fps", Some(m)) => cmd_fps(&brp, parse_f64(m, "settle-secs")),
        ("get", Some(_)) => brp.get_settings().map(|settings| {
            println!("{}", serde_json::to_string_pretty(&settings).unwrap());
        }),
        ("set", Some(m)) => cmd_set(
            &brp,
            m.value_of("field").unwrap(),
            m.value_of("value").unwrap(),
        ),
        ("attribute", Some(m)) => cmd_attribute(&brp, parse_f64(m, "secs")),
        _ => unreachable!("SubcommandRequiredElseHelp"),
    };

    if let Err(message) = result {
        exit_with(&message);
    }
}

fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
