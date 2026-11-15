//! JSON/worker baseline; same package, toolchain and release settings as the probe.
use std::io::{self, Read};
fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let value: serde_json::Value = serde_json::from_str(&input).unwrap();
    let output = std::thread::spawn(
        move || serde_json::json!({"commits": value["commits"].as_array().unwrap().len()}),
    )
    .join()
    .unwrap();
    println!("{output}");
}
