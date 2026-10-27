use fuser::BackgroundSession;
use log::info;
use nix::sys::signal::{SigSet, Signal};
use nix::{mount, unistd};
use simple_error::bail;
use simple_error::try_with;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use crate::fs::EnvFs;
use crate::logger::enable_debug_log;
use crate::result::Result;

mod fs;
mod logger;
mod result;
mod setrlimit;

/// Unmounts on drop, but only mountpoints still backed by our own FUSE
/// connection: after a restart, a newer daemon may own the same path.
struct MountGuard<'a> {
    mountpoints: &'a [PathBuf],
    dev: u64,
}

struct Options {
    mountpoints: Vec<PathBuf>,
    debug: bool,
    show_help: bool,
    foreground: bool,
    remount: bool,
    fallback_paths: Vec<PathBuf>,
    args: Vec<String>,
}

fn shutdown_signals() -> SigSet {
    let mut sigset = SigSet::empty();
    sigset.add(Signal::SIGINT);
    sigset.add(Signal::SIGTERM);
    sigset
}

/// Blocks until SIGINT/SIGTERM arrives or the FUSE session ends because the
/// kernel unmounted us; otherwise the daemon would outlive its mount.
fn wait_for_shutdown(session: BackgroundSession) -> Result<()> {
    let (tx, rx) = mpsc::channel();

    let sigset = shutdown_signals();
    let signal_tx = tx.clone();
    thread::spawn(move || {
        if let Ok(sig) = sigset.wait() {
            info!("Received {sig}");
        }
        let _ = signal_tx.send(());
    });
    thread::spawn(move || {
        if let Err(e) = session.join() {
            info!("FUSE session ended: {e}");
        }
        let _ = tx.send(());
    });

    try_with!(rx.recv(), "shutdown channel closed");
    Ok(())
}

fn serve_fs(opts: &Options) -> Result<()> {
    let mut notify = None;
    if !opts.foreground {
        // We need to make sure the foreground process does not exit until we have successfully
        // mounted the file system (otherwise, systemd will fail with protocol error).
        //
        // We do this by creating a pipe. The pipe can be closed by either the child process exiting,
        // or the child process closing it (which signals successfully mounting).
        let (mut parent, child) = try_with!(std::io::pipe(), "cannot create pipe");
        // SAFETY: we're still single threaded at this point.
        match try_with!(unsafe { unistd::fork() }, "cannot fork") {
            unistd::ForkResult::Child => {
                drop(parent);
                // Create a new session to detach from the controlling
                // terminal, matching the behaviour of daemon(3).
                let _ = unistd::setsid();
                notify = Some(child);
            }
            unistd::ForkResult::Parent { .. } => {
                drop(child);
                if try_with!(parent.read(&mut [0]), "cannot read from pipe") == 0 {
                    // In this case, the child process exited before notifying us.
                    bail!("child process exited");
                }
                info!("Background process ready");
                return Ok(());
            }
        }
    }

    let fs = try_with!(
        EnvFs::new(opts.fallback_paths.as_slice()),
        "cannot create filesystem"
    );

    // Block before any thread is spawned so every thread inherits the mask
    // and the signals are only consumed by sigwait() in wait_for_shutdown.
    try_with!(shutdown_signals().thread_block(), "cannot block signals");

    let session = try_with!(fs.mount(&opts.mountpoints), "cannot start fuse sessions");
    let dev = try_with!(
        std::fs::metadata(&opts.mountpoints[0]),
        "cannot stat {}",
        opts.mountpoints[0].display()
    )
    .dev();
    let guard = MountGuard {
        mountpoints: &opts.mountpoints,
        dev,
    };

    // Notify the foreground process on successful mount.
    if let Some(mut notify) = notify {
        info!("Notifying foreground process");
        // Release stdin/stdout/stderr and cwd.
        _ = std::env::set_current_dir("/");
        if let Ok(null) = std::fs::File::open("/dev/null") {
            _ = unistd::dup2_stdin(&null);
            _ = unistd::dup2_stdout(&null);
            _ = unistd::dup2_stderr(&null);
        }
        _ = notify.write_all(&[0]);
    }

    wait_for_shutdown(session)?;
    info!("Stop fuse");
    drop(guard);

    Ok(())
}

impl Drop for MountGuard<'_> {
    fn drop(&mut self) {
        for mountpoint in self.mountpoints {
            match std::fs::metadata(mountpoint) {
                Ok(m) if m.dev() == self.dev => {
                    let _ = mount::umount(mountpoint);
                }
                _ => info!("{} is no longer ours, skip unmount", mountpoint.display()),
            }
        }
    }
}

fn show_help(prog_name: &str) {
    eprintln!("USAGE: {prog_name} [options] mountpoint");
    eprintln!("-h, --help             show help");
    eprintln!("-f, --foreground       do not daemonize");
    eprintln!("-o debug               debug logging");
    eprintln!("-o fallback-path=PATH  Fallback path if PATH is not set");
    eprintln!("                       (can be passed multiple times)");
    eprintln!("-o bind-mount=PATH     Bind mount PATH with envfs");
    eprintln!("                       (can be passed multiple times)");
}

fn parse_mount_options(mount_options: &str, opts: &mut Options) -> Result<()> {
    for option in mount_options.split(',') {
        let mount_opt: Vec<&str> = option.splitn(2, '=').collect();
        match mount_opt[0] {
            // ignore
            "ro" | "rw" | "nofail" => {}
            "remount" => {
                opts.remount = true;
            }
            "debug" => {
                opts.debug = true;
            }
            "bind-mount" => {
                if mount_opt.len() != 2 {
                    bail!("bind-mount needs an argument");
                }
                opts.mountpoints.push(PathBuf::from(mount_opt[1]));
            }
            "fallback-path" => {
                if mount_opt.len() != 2 {
                    bail!("fallback-path needs an argument");
                }
                opts.fallback_paths.push(PathBuf::from(mount_opt[1]));
            }
            _ => {
                eprintln!("ignore invalid mount option: {}", mount_opt[0]);
            }
        }
    }
    Ok(())
}

fn parse_options(args: &[String]) -> Result<Options> {
    let mut i: usize = 0;
    let mut opts = Options {
        mountpoints: vec![],
        debug: false,
        show_help: false,
        foreground: false,
        remount: false,
        fallback_paths: vec![],
        args: vec![],
    };
    loop {
        if i >= args.len() {
            return Ok(opts);
        }
        match args[i].as_ref() {
            "-h" | "--help" => {
                opts.show_help = true;
                return Ok(opts);
            }
            "-f" | "--foreground" => {
                opts.foreground = true;
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    bail!("'-o' requires an argument");
                }
                parse_mount_options(&args[i], &mut opts)?;
            }
            _ => {
                if args[i].starts_with('-') && args[i] != "--" {
                    bail!("unrecognized argument '{}'", args[i]);
                }
                if args[i] == "--" {
                    opts.args.extend_from_slice(&args[i + 1..]);
                    return Ok(opts);
                }
                opts.args.push(String::from(args[i].as_str()));
            }
        }
        i += 1;
    }
}

fn run_app(args: &[String]) -> i32 {
    let default_name = String::from("envfs");
    let app_name = args.first().unwrap_or(&default_name);
    let mut opts = match parse_options(&args[1..]) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("{app_name}: {err}");
            return 1;
        }
    };
    if opts.args.is_empty() {
        eprintln!("Not enough arguments.");
        show_help(app_name);
        return 1;
    }
    opts.mountpoints.insert(
        0,
        PathBuf::from(&opts.args[usize::from(opts.args.len() != 1)]),
    );

    if opts.show_help {
        show_help(app_name);
        return 0;
    }
    if opts.remount {
        eprintln!("Ignoring remount request.");
        return 0;
    }
    if opts.debug {
        if let Err(err) = enable_debug_log() {
            eprintln!("{app_name}: cannot set up logging: {err}");
        }
    }

    match serve_fs(&opts) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(run_app(&args))
}
