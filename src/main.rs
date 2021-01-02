use lazy_static::lazy_static;
use nix::mount;
use nix::sys::signal;
use simple_error::bail;
use simple_error::try_with;
use std::path::{Path, PathBuf};
use std::sync::Condvar;
use std::sync::Mutex;
use log::info;

use crate::fs::EnvFs;
use crate::logger::enable_debug_log;
use crate::result::Result;

mod fs;
mod fusefd;
mod logger;
mod num_cpus;
mod result;
mod setrlimit;

struct MountGuard<'a> {
    mount_point: &'a Path,
}

lazy_static! {
    static ref SIGNAL_RECEIVED: Condvar = Condvar::new();
}

extern "C" fn handle_sigint(_: i32) {
    SIGNAL_RECEIVED.notify_all();
}

fn serve_fs(mountpoint: &Path) -> Result<()> {
    let fs = try_with!(EnvFs::new(), "cannot create filesystem");
    try_with!(fs.mount(mountpoint), "cannot mount filesystem");

    let guard = MountGuard {
        mount_point: mountpoint,
    };
    let sessions = try_with!(fs.spawn_sessions(), "cannot start fuse sessions");

    let sig_action = signal::SigAction::new(
        signal::SigHandler::Handler(handle_sigint),
        signal::SaFlags::empty(),
        signal::SigSet::empty(),
    );

    unsafe {
        try_with!(
            signal::sigaction(signal::SIGINT, &sig_action),
            "Unable to register SIGINT handler"
        );
        try_with!(
            signal::sigaction(signal::SIGTERM, &sig_action),
            "Unable to register SIGTERM handler"
        );
    }

    let mutex = Mutex::new(true);
    let lock_result = try_with!(mutex.lock(), "cannot acquire lock");
    let res = try_with!(
        SIGNAL_RECEIVED.wait(lock_result),
        "failed to wait for signal barrier"
    );
    info!("Stop fuse");

    drop(guard);
    for session in sessions {
        let _ = session.join();
    }
    drop(res);

    Ok(())
}

impl<'a> Drop for MountGuard<'a> {
    fn drop(&mut self) {
        let _ = mount::umount(self.mount_point);
    }
}

struct Options<'a> {
    verbose: bool,
    show_help: bool,
    args: &'a [String],
}

fn show_help(prog_name: &str) {
    eprintln!("USAGE: {} [options] mountpoint", prog_name);
    eprintln!("-h, --help     show help");
    eprintln!("-v, --verbose  verbose logging");
}

fn parse_options(args: &[String]) -> Result<Options> {
    let mut i: usize = 0;
    let mut opts = Options {
        verbose: false,
        show_help: false,
        args: &[],
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
            "-v" | "--verbose" => {
                opts.verbose = true;
            }
            _ => {
                if args[i].starts_with("-") && args[i] != "--" {
                    bail!("unrecognized argument '{}'", args[i]);
                }
                if args[i] == "--" {
                    opts.args = &args[i + 1..];
                } else {
                    opts.args = &args[i..];
                }
                return Ok(opts);
            }
        }
        i += 1;
    }
}

fn run_app(args: &[String]) -> i32 {
    let default_name = String::from("envfs");
    let app_name = args.get(0).unwrap_or(&default_name);
    let opts = match parse_options(&args[1..]) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("{}: {}", app_name, err);
            return 1;
        }
    };
    if opts.args.len() == 0 {
        eprintln!("Not enough arguments.");
        show_help(app_name);
        return 1;
    }
    if opts.show_help {
        show_help(app_name);
        return 0;
    }
    if opts.verbose {
        if let Err(err) = enable_debug_log() {
            eprintln!("{}: cannot set up logging: {}", app_name, err);
        }
    }

    let mountpoint = &opts.args[0];

    match serve_fs(&PathBuf::from(mountpoint)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    return 0;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(run_app(&args))
}
