//! Cached presentation adapter for the experiment; never linked into Tracer.
use serde_json::{json, Value};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq)]
struct Span {
    text: String,
    color: Option<u8>,
}
fn graph_spans(line: &str) -> Result<Vec<Span>, String> {
    let mut remaining = line;
    let mut color = None;
    let mut spans = Vec::new();
    while !remaining.is_empty() {
        if remaining.starts_with('\x1b') {
            let end = remaining.find('m').ok_or("unterminated colour")?;
            let code = &remaining[..=end];
            if code == "\x1b[0m" {
                color = None;
            } else {
                let number = code
                    .strip_prefix("\x1b[38;5;")
                    .and_then(|s| s.strip_suffix('m'))
                    .ok_or("unexpected escape")?;
                color = Some(number.parse::<u8>().map_err(|e| e.to_string())?);
            }
            remaining = &remaining[end + 1..];
        } else {
            let end = remaining.find('\x1b').unwrap_or(remaining.len());
            spans.push(Span {
                text: remaining[..end].to_owned(),
                color,
            });
            remaining = &remaining[end..];
        }
    }
    Ok(spans)
}
fn clip(spans: &[Span], offset: usize, width: usize) -> Vec<Span> {
    let mut column = 0;
    let mut clipped = Vec::new();
    let end = offset.saturating_add(width);
    for span in spans {
        let mut text = String::new();
        for glyph in span.text.graphemes(true) {
            let next = column + glyph.width();
            if column >= end {
                break;
            }
            if next > offset {
                if column >= offset && next <= end {
                    text.push_str(glyph);
                } else {
                    text.push_str(&" ".repeat(next.min(end).saturating_sub(column.max(offset))));
                }
            }
            column = next;
        }
        if !text.is_empty() {
            clipped.push(Span {
                text,
                color: span.color,
            });
        }
    }
    clipped
}
pub fn attach(output: &mut Value, input: &Value) -> Result<(), String> {
    let Some(requests) = input["viewports"].as_array() else {
        return Ok(());
    };
    if output["status"] != "rendered" {
        return Ok(());
    }
    let mut rows: Vec<Vec<Span>> = output["lines"]
        .as_array()
        .ok_or("missing rows")?
        .iter()
        .map(|line| graph_spans(line.as_str().ok_or("invalid row")?))
        .collect::<Result<_, _>>()?;
    let graph_width = rows
        .iter()
        .map(|row| row.iter().map(|span| span.text.width()).sum::<usize>())
        .max()
        .unwrap_or(0);
    let positions = output["positions"].as_array().ok_or("missing positions")?;
    let commits: std::collections::HashMap<_, _> = input["commits"]
        .as_array()
        .ok_or("missing commits")?
        .iter()
        .filter_map(|commit| commit["oid"].as_str().map(|oid| (oid, commit)))
        .collect();
    for position in positions {
        let index = position["row"].as_u64().ok_or("invalid commit row")? as usize;
        let row = rows.get_mut(index).ok_or("commit outside rows")?;
        let oid = position["oid"].as_str().ok_or("missing oid")?;
        let commit = commits.get(oid).ok_or("unknown oid")?;
        let width: usize = row.iter().map(|span| span.text.width()).sum();
        row.push(Span {
            text: format!(
                "{}{} {}",
                " ".repeat(graph_width.saturating_sub(width) + 2),
                &oid[..7.min(oid.len())],
                commit["subject"].as_str().unwrap_or("")
            ),
            color: None,
        });
    }
    let views: Vec<_> = requests.iter().map(|request| {
        let x = request["x"].as_u64().unwrap_or(0) as usize;
        let y = request["y"].as_u64().unwrap_or(0) as usize;
        let width = request["width"].as_u64().unwrap_or(80) as usize;
        let height = request["height"].as_u64().unwrap_or(24) as usize;
        let visible: Vec<_> = rows.iter().skip(y).take(height).map(|row| clip(row,x,width).iter().map(|span| json!({"text":span.text,"color":span.color})).collect::<Vec<_>>()).collect();
        let mapping: Vec<_> = positions.iter().filter_map(|p| {
            let r = p["row"].as_u64()? as usize;
            (r >= y && r < y.saturating_add(height)).then(|| json!({"oid":p["oid"],"row":r-y,"node_visible":p["column"].as_u64().is_some_and(|c| c as usize >= x && (c as usize) < x.saturating_add(width))}))
        }).collect();
        json!({"request":request,"rows":visible,"positions":mapping})
    }).collect();
    output["viewports"] = json!(views);
    output["render_passes"] = json!(1);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipping_preserves_graphemes_and_pads_partial_wide_glyphs() {
        let spans = vec![Span {
            text: "e\u{301}界👩‍💻X".into(),
            color: Some(6),
        }];
        assert_eq!(clip(&spans, 0, 3)[0].text, "e\u{301}界");
        assert_eq!(clip(&spans, 2, 3)[0].text, " 👩‍💻");
        assert_eq!(clip(&spans, 4, 2)[0].text, " X");
        assert!(clip(&spans, 0, 0).is_empty());
    }
    #[test]
    fn repeated_cached_slices_are_identical_and_do_not_mutate_input() {
        let spans = graph_spans("\x1b[38;5;6m╭─●\x1b[0m label").unwrap();
        let before = spans.clone();
        assert_eq!(clip(&spans, 1, 4), clip(&spans, 1, 4));
        assert_eq!(spans, before);
        assert!(graph_spans("\x1b[2J").is_err());
    }
}
