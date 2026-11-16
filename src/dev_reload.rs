//! Private handshake for the development runner; inactive in ordinary launches.
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct DevReload {
    directory: Option<PathBuf>,
    next_check: Instant,
    requested: bool,
}

impl DevReload {
    pub fn new() -> io::Result<Self> {
        let directory = std::env::var_os("TRACR_DEV_CONTROL").map(PathBuf::from);
        if let Some(path) = &directory {
            std::fs::write(path.join("ready"), b"")?;
        }
        Ok(Self {
            directory,
            next_check: Instant::now(),
            requested: false,
        })
    }

    pub fn requested(&mut self) -> bool {
        if !self.requested && self.directory.is_some() && Instant::now() >= self.next_check {
            self.next_check = Instant::now() + Duration::from_millis(500);
            self.requested = self
                .directory
                .as_ref()
                .is_some_and(|path| path.join("request").exists());
        }
        self.requested
    }

    pub fn acknowledge(&self) -> io::Result<()> {
        if let Some(path) = &self.directory {
            std::fs::write(path.join("ack"), b"")?;
        }
        Ok(())
    }
}

pub fn exit_code() -> u8 {
    if std::env::var_os("TRACR_DEV_CONTROL")
        .is_some_and(|path| PathBuf::from(path).join("ack").exists())
    {
        75
    } else {
        0
    }
}
