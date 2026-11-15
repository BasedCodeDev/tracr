//! Isolated compiled experiment. Nothing here is linked into Tracer.
mod viewport;
use gleisbau::{
    graph::Oid,
    layout::layout_track_range,
    print::{format::CommitFormat, unicode::print_graph_terminal},
    settings::{
        BranchOrder, BranchSettings, BranchSettingsDef, Characters, MergePatterns, Settings,
    },
    track::{Builder, TrackMap},
};
use serde_json::{json, Value};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::RefCell,
    io::{self, Read},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

struct Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(p, layout);
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn settings(order: &str) -> Settings {
    let mut branches = BranchSettings::from(BranchSettingsDef::none()).unwrap();
    // The library indexes unknown colours from one, hence rotate the palette.
    branches.terminal_colors_unknown = ["red", "cyan", "magenta", "yellow", "green", "blue"]
        .map(str::to_owned)
        .to_vec();
    Settings {
        reverse_commit_order: false,
        debug: false,
        compact: true,
        colored: true,
        include_remote: true,
        format: CommitFormat::OneLine,
        wrapping: None,
        characters: Characters::round(),
        branch_order: match order {
            "shortest-forward" => BranchOrder::ShortestFirst(true),
            "longest-forward" => BranchOrder::LongestFirst(true),
            "longest-reverse" => BranchOrder::LongestFirst(false),
            _ => BranchOrder::ShortestFirst(false),
        },
        branches,
        merge_patterns: MergePatterns::default(),
    }
}

fn render(input: &Value) -> Result<Value, String> {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let start = Instant::now();
    let settings = settings(input["order"].as_str().unwrap_or("shortest-reverse"));
    let tracks = Rc::new(RefCell::new(TrackMap::<Oid>::new()));
    let mut builder = Builder::new(tracks.clone());
    let commits = input["commits"]
        .as_array()
        .ok_or("commits must be an array")?;
    for c in commits {
        let oid =
            Oid::from_str(c["oid"].as_str().ok_or("missing oid")?).map_err(|e| e.to_string())?;
        let parents = c["parents"]
            .as_array()
            .ok_or("missing parents")?
            .iter()
            .map(|p| Oid::from_str(p.as_str().unwrap_or("")))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        builder.add_commit(oid, c["subject"].as_str().unwrap_or("").to_owned(), parents);
    }
    let mut tracks = tracks.borrow_mut();
    // Public completion APIs, no repository traversal and no upstream patches.
    let indices = tracks.indices.clone();
    gleisbau::backend::git2::assign_children(&mut tracks.commits, &indices);
    if let Some(refs) = input["refs"].as_object() {
        for branch in &mut tracks.all_branches {
            if let Some(name) = refs.get(&branch.target.to_string()).and_then(Value::as_str) {
                branch.name = name.to_owned();
            }
        }
    }
    // Anonymous tracks use persistence 254; the drawing grid starts at 2.
    // Normalize to this experiment's single, equal-priority branch group.
    for branch in &mut tracks.all_branches {
        branch.persistence = 0;
    }
    {
        let t = &mut *tracks;
        gleisbau::backend::git2::assign_sources_targets(
            &t.commits,
            &t.indices,
            &mut t.all_branches,
        );
    }
    let ingest_us = start.elapsed().as_micros();
    let layout_start = Instant::now();
    let layout = layout_track_range(&tracks, 0..tracks.commits.len(), &settings)?;
    let layout_us = layout_start.elapsed().as_micros();
    let draw_start = Instant::now();
    let mut heights = vec![1; commits.len()];
    // Reserve one bottom cell for incomplete ancestry; true roots get no tail.
    if tracks
        .commits
        .iter()
        .any(|c| c.parents.iter().any(|p| !tracks.indices.contains_key(p)))
    {
        if let Some(last) = heights.last_mut() {
            *last = 2;
        }
    }
    let graph = print_graph_terminal(&settings, &tracks, &layout, &heights);
    let draw_us = draw_start.elapsed().as_micros();
    let retained = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    let positions: Vec<_> = tracks
        .commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let visual = c.branch_trace.and_then(|b| layout.track_visual(b));
            json!({"oid":c.oid.to_string(), "row":graph.commit2line.get(i),
               "column":visual.and_then(|v|v.column).map(|x|x*2),
               "color":visual.map(|v|v.term_color)})
        })
        .collect();
    Ok(
        json!({"status":"rendered", "lines":graph.graph_lines, "positions":positions,
              "timing_us":{"ingest":ingest_us,"layout":layout_us,"render":draw_us},
              "retained_working_bytes":retained,"peak_working_bytes":peak}),
    )
}

fn main() {
    let mut data = String::new();
    io::stdin().read_to_string(&mut data).unwrap();
    let input: Value = serde_json::from_str(&data).expect("valid input JSON");
    // All Rc/library state stays inside this worker; only owned JSON leaves it.
    let viewport_input = input.get("viewports").map(|_| input.clone());
    let worker =
        std::thread::spawn(
            move || match catch_unwind(AssertUnwindSafe(|| render(&input))) {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => json!({"status":"error", "error":error}),
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    json!({"status":"panic", "error":message})
                }
            },
        );
    let mut output = worker.join().expect("worker result");
    if let Some(input) = viewport_input {
        if let Err(error) = viewport::attach(&mut output, &input) {
            output["viewport_error"] = json!(error);
        }
    }
    println!("{output}");
}
