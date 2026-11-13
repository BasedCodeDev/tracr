//! Linux foreground-process handoff. Only the newly spawned process group is supervised.
use crate::domain::LaunchCommand;
use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

fn syscall(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A background parent must block SIGTTOU while reclaiming the terminal. Restore
/// the calling thread's original signal mask immediately after the operation.
fn set_foreground(fd: libc::c_int, group: libc::pid_t) -> io::Result<()> {
    // SAFETY: all pointers refer to initialized, stack-owned sigset_t values;
    // the descriptor is held open by Foreground for the duration of this call.
    unsafe {
        let mut blocked = std::mem::zeroed();
        let mut previous = std::mem::zeroed();
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        let error = libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous);
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        let result = syscall(libc::tcsetpgrp(fd, group));
        let error = libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        result?;
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
    }
    Ok(())
}
struct Foreground {
    tty: File,
    group: libc::pid_t,
    attributes: libc::termios,
}
impl Foreground {
    fn capture() -> io::Result<Self> {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        // SAFETY: tcgetattr fills the supplied termios, with a valid tty descriptor.
        let (group, attributes) = unsafe {
            let group = libc::tcgetpgrp(tty.as_raw_fd());
            if group == -1 {
                return Err(io::Error::last_os_error());
            }
            let mut attributes = std::mem::zeroed();
            syscall(libc::tcgetattr(tty.as_raw_fd(), &mut attributes))?;
            (group, attributes)
        };
        Ok(Self {
            tty,
            group,
            attributes,
        })
    }
    fn restore(&self) -> io::Result<()> {
        set_foreground(self.tty.as_raw_fd(), self.group)?;
        // SAFETY: the captured attributes and tty remain valid for this guard.
        unsafe {
            syscall(libc::tcsetattr(
                self.tty.as_raw_fd(),
                libc::TCSANOW,
                &self.attributes,
            ))
        }
    }
}
impl Drop for Foreground {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct OwnedChild {
    child: Child,
    group: libc::pid_t,
    reaped: bool,
}
impl OwnedChild {
    fn signal(&self, signal: libc::c_int) -> io::Result<()> {
        // SAFETY: group is the positive PID of our own newly spawned process-group
        // leader. A negative PID targets precisely that group, never our group.
        let result = unsafe { libc::kill(-self.group, signal) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.signal(libc::SIGKILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Called only after tracr has suspended raw mode and its alternate screen.
/// Polling std::process::Child avoids relying on asynchronous SIGCHLD delivery.
pub async fn run(command: &LaunchCommand, terminate: &AtomicBool) -> io::Result<ExitStatus> {
    let foreground = Foreground::capture()?;
    if terminate.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Termination requested before launch",
        ));
    }
    let child = Command::new(&command.executable)
        .args(&command.args)
        .envs(command.env.iter().cloned())
        .current_dir(&command.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()?;
    let group = libc::pid_t::try_from(child.id()).expect("Linux PID fits pid_t");
    let mut owned = OwnedChild {
        child,
        group,
        reaped: false,
    };
    if let Err(error) = set_foreground(foreground.tty.as_raw_fd(), group) {
        if let Some(status) = owned.child.try_wait()? {
            owned.reaped = true;
            foreground.restore()?;
            return Ok(status);
        }
        return Err(error);
    }
    // A child that read before tcsetpgrp may have received SIGTTIN. Continuing
    // after foreground ownership is established also handles that startup race.
    owned.signal(libc::SIGCONT)?;
    let mut stopping_at = None;
    let mut killed = false;
    let status = loop {
        if terminate.load(Ordering::Relaxed) && stopping_at.is_none() {
            owned.signal(libc::SIGTERM)?;
            owned.signal(libc::SIGCONT)?;
            stopping_at = Some(Instant::now());
        }
        if !killed && stopping_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(5)) {
            owned.signal(libc::SIGKILL)?;
            killed = true;
        }
        if let Some(status) = owned.child.try_wait()? {
            owned.reaped = true;
            // Termination also cleans up any remaining members of the owned group.
            if stopping_at.is_some() {
                owned.signal(libc::SIGKILL)?;
            }
            break status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    foreground.restore()?;
    Ok(status)
}
