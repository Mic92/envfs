use lazy_static::lazy_static;
use log::info;
use nix::sys::signal;
use nix::{mount, unistd};
use simple_error::bail;
use simple_error::try_with;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};

use crate::fs::EnvFs;
use crate::logger::enable_debug_log;
use crate::result::Result;

mod fs;
mod logger;
mod result;
mod setrlimit;

struct MountGuard<'a> {
    mountpoints: &'a [PathBuf],
}

lazy_static! {
    static ref SIGNAL_RECEIVED: Condvar = Condvar::new();
}

extern "C" fn handle_sigint(_: i32) {
    SIGNAL_RECEIVED.notify_all();
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

fn wait_signal(mountpoints: &[PathBuf]) -> Result<()> {
    let guard = MountGuard { mountpoints };

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

    let mutex = Mutex::new(());
    let lock_result = try_with!(mutex.lock(), "cannot acquire lock");
    let res = try_with!(
        SIGNAL_RECEIVED.wait(lock_result),
        "failed to wait for signal barrier"
    );
    info!("Stop fuse");

    drop(guard);
    drop(res);

    Ok(())
}

fn serve_fs(opts: &Options) -> Result<()> {
    if !opts.foreground {
        try_with!(unistd::daemon(true, true), "cannot daemonize");
    }

    let fs = try_with!(
        EnvFs::new(opts.fallback_paths.as_slice()),
        "cannot create filesystem"
    );

    let session = try_with!(fs.mount(&opts.mountpoints), "cannot start fuse sessions");

    wait_signal(&opts.mountpoints)?;
    drop(session);

    Ok(())
}

impl Drop for MountGuard<'_> {
    fn drop(&mut self) {
        for mountpoint in self.mountpoints {
            let _ = mount::umount(mountpoint);
        }
    }
}

fn show_help(prog_name: &str) {
    eprintln!("USAGE: {} [options] mountpoint", prog_name);
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
            eprintln!("{}: {}", app_name, err);
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
            eprintln!("{}: cannot set up logging: {}", app_name, err);
        }
    }

    match serve_fs(&opts) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(run_app(&args))
}
