//! Explicit, asynchronous copying to native Linux desktop clipboards.
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

fn backend(wayland: bool, x11: bool) -> Result<(&'static str, &'static [&'static str]), String> {
    if wayland {
        Ok(("wl-copy", &["--type", "text/plain;charset=utf-8"]))
    } else if x11 {
        Ok(("xclip", &["-selection", "clipboard"]))
    } else {
        Err("Copy unavailable: no Wayland or X11 desktop clipboard.".into())
    }
}

pub async fn copy(text: String) -> Result<(), String> {
    let present = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    let (program, args) = backend(present("WAYLAND_DISPLAY"), present("DISPLAY"))?;
    run(program, args, &text, Duration::from_secs(3)).await
}

async fn run(program: &str, args: &[&str], text: &str, timeout: Duration) -> Result<(), String> {
    let operation = async {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    format!("Copy unavailable: install {program} to use the desktop clipboard.")
                } else {
                    format!("Could not start {program}: {e}")
                }
            })?;
        let mut input = child.stdin.take().expect("piped clipboard input");
        input
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("Could not send message to {program}: {e}"))?;
        drop(input);
        let status = child
            .wait()
            .await
            .map_err(|e| format!("Could not wait for {program}: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "Copy failed: {program} exited with {status}. Check the desktop clipboard connection."
            ))
        }
    };
    tokio::time::timeout(timeout, operation)
        .await
        .unwrap_or_else(|_| Err(format!("Copy timed out: {program} did not finish.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_native_desktop_backend() {
        assert_eq!(backend(true, true).unwrap().0, "wl-copy");
        assert_eq!(
            backend(false, true).unwrap(),
            ("xclip", &["-selection", "clipboard"][..])
        );
        assert!(backend(false, false).is_err());
    }
    #[tokio::test]
    async fn sends_exact_text_and_reports_failures() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("copied.txt");
        let text = "Couldn't relink session\n\nUnicode: 路径 é\nOriginal folder: /long path/$literal\n\nTechnical details: test";
        run(
            "/usr/bin/python3",
            &[
                "-c",
                "import sys;open(sys.argv[1],'wb').write(sys.stdin.buffer.read())",
                out.to_str().unwrap(),
            ],
            text,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(out).unwrap(), text);
        assert!(
            run(
                "/nonexistent/tracr-clipboard",
                &[],
                text,
                Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .contains("install")
        );
        assert!(
            run(
                "/usr/bin/python3",
                &["-c", "import sys;sys.stdin.read();sys.exit(4)"],
                text,
                Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .contains("exited")
        );
        assert!(
            run(
                "/usr/bin/python3",
                &["-c", "import time;time.sleep(10)"],
                text,
                Duration::from_millis(50)
            )
            .await
            .unwrap_err()
            .contains("timed out")
        );
    }
}
