//! Reproducible previews from the actual renderer; no repository access or UI changes.
//! cargo run --example git_graph_preview -- target/git-graph-preview
#[path = "../tests/support/git_graph_fixtures.rs"]
mod fixtures;
use std::{fmt::Write, path::PathBuf};
use tracr::{git_layout::layout, git_render::render};

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn css(color: Option<u8>) -> &'static str {
    match color {
        Some(31) => "#e06c75",
        Some(32) => "#98c379",
        Some(33) => "#e5c07b",
        Some(34) => "#61afef",
        Some(35) => "#c678dd",
        Some(36) => "#56b6c2",
        _ => "#abb2bf",
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dest = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .unwrap_or_else(|| "target/git-graph-preview".into()),
    );
    std::fs::create_dir_all(&dest)?;
    let mut cases = fixtures::fixtures();
    let mut boundary = fixtures::wide(12, 4);
    boundary.truncate(5);
    cases.push(("unresolved", boundary));
    for (name, commits) in cases {
        let model = layout(&commits)?;
        let graph = render(&model);
        let rows: Vec<_> = graph.rows.iter().chain(graph.boundary.iter()).collect();
        let width = (graph.width + 56) * 10 + 40;
        let height = (rows.len() + 5) * 22 + 20;
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><rect width=\"100%\" height=\"100%\" fill=\"#171b22\"/><g font-family=\"DejaVu Sans Mono, monospace\" font-size=\"16\"><text x=\"20\" y=\"28\" fill=\"#ffffff\">Unicode renderer · {name}</text>"
        );
        let mut ansi = format!("Unicode renderer · {name}\n\n");
        let mut plain = format!("Unicode renderer · {name}\n\n");
        for (y, row) in rows.iter().enumerate() {
            let selected = row
                .oid
                .as_ref()
                .is_some_and(|oid| commits.get(1).is_some_and(|c| c.oid == oid.as_ref()));
            let baseline = 72 + y * 22;
            if selected {
                write!(
                    svg,
                    "<rect x=\"14\" y=\"{}\" width=\"{}\" height=\"22\" fill=\"#2c3442\"/>",
                    baseline - 17,
                    width - 28
                )?;
                ansi.push_str("\x1b[48;5;236m");
            }
            let cursor = if selected { "› " } else { "  " };
            plain.push_str(cursor);
            ansi.push_str(cursor);
            write!(
                svg,
                "<text x=\"20\" y=\"{baseline}\" fill=\"#ffffff\">{}</text>",
                escape(cursor)
            )?;
            let mut x = 40;
            for span in &row.spans {
                write!(ansi, "\x1b[{}m{}", span.color.unwrap_or(39), span.text)?;
                plain.push_str(&span.text);
                for ch in span.text.chars() {
                    if ch != ' ' {
                        write!(
                            svg,
                            "<text x=\"{x}\" y=\"{baseline}\" fill=\"{}\" textLength=\"10\" lengthAdjust=\"spacingAndGlyphs\">{}</text>",
                            css(span.color),
                            escape(&ch.to_string())
                        )?;
                    }
                    x += 10;
                }
            }
            let padding = " ".repeat(graph.width - row.text().chars().count() + 2);
            let label = if let Some(oid) = &row.oid {
                let i = commits.iter().position(|c| c.oid == oid.as_ref()).unwrap();
                let reference = match i {
                    0 => "[HEAD → main] ",
                    1 => "[feature/navigation] ",
                    2 => "[release] ",
                    3 => "[fix/session] ",
                    _ => "",
                };
                let subject = if commits[i].parents.len() > 1 {
                    "Merge branches"
                } else if commits[i].parents.is_empty() {
                    "Initial commit"
                } else {
                    "Update project"
                };
                format!("{} {reference}{subject}", &oid[oid.len() - 7..])
            } else if y == graph.rows.len() {
                "older history not loaded".into()
            } else {
                String::new()
            };
            writeln!(ansi, "{padding}\x1b[39m{label}\x1b[0m")?;
            writeln!(plain, "{padding}{label}")?;
            if !label.is_empty() {
                write!(
                    svg,
                    "<text x=\"{}\" y=\"{baseline}\" fill=\"#abb2bf\">{}</text>",
                    40 + (graph.width + 2) * 10,
                    escape(&label)
                )?;
            }
        }
        write!(
            svg,
            "<text x=\"20\" y=\"{}\" fill=\"#abb2bf\" font-size=\"12\">› cursor · HEAD reference · ⌢ crossing, no join · ┊ unloaded ancestry</text></g></svg>",
            height - 12
        )?;
        std::fs::write(dest.join(format!("{name}.svg")), svg)?;
        std::fs::write(dest.join(format!("{name}.ansi")), ansi)?;
        std::fs::write(dest.join(format!("{name}.txt")), plain)?;
        println!(
            "{name}: {} completed rows, {} cells wide",
            graph.rows.len(),
            graph.width
        );
    }
    Ok(())
}
