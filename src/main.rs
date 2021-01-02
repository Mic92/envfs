use simple_error::try_with;
use std::env;
use std::path::{Path, PathBuf};
use std::process::exit;
use nix::mount;
use nix::sys::signal;
use std::sync::{Mutex};
use std::sync::Condvar;
use lazy_static::lazy_static;

use crate::fs::EnvFs;
use crate::logger::enable_debug_log;
use crate::result::Result;

mod fs;
mod fusefd;
mod logger;
mod result;
mod setrlimit;
mod num_cpus;


struct MountGuard<'a> {
    mount_point: &'a Path,
}

lazy_static! {
    static ref SIGNAL_HAPPEN : Condvar = {
        Condvar::new()
    };
}

extern "C" fn handle_sigint(_: i32) {
    SIGNAL_HAPPEN.notify_all();
}


fn mount_fs(mountpoint: &Path) -> Result<()> {
    let fs = try_with!(EnvFs::new(), "cannot create filesystem");
    try_with!(fs.mount(mountpoint), "cannot mount filesystem");

    let guard = MountGuard {
        mount_point: mountpoint,
    };
    let sessions = fs.spawn_sessions().unwrap();

    let sig_action = signal::SigAction::new(
        signal::SigHandler::Handler(handle_sigint),
        signal::SaFlags::empty(),
        signal::SigSet::empty(),
    );

    unsafe {
        try_with!(signal::sigaction(signal::SIGINT, &sig_action),
                  "Unable to register SIGINT handler");
        try_with!(signal::sigaction(signal::SIGTERM, &sig_action),
                  "Unable to register SIGTERM handler");
    }

    let mutex = Mutex::new(true);
    let lock_result = try_with!(mutex.lock(), "cannot acquire lock");
    let _ = try_with!(SIGNAL_HAPPEN.wait(lock_result),
              "failed to wait for signal barrier");

    drop(guard);
    for session in sessions {
        let _ = session.join();
    }

    Ok(())
}

impl<'a> Drop for MountGuard<'a> {
    fn drop(&mut self) {
        let _ = mount::umount(self.mount_point);
    }
}

fn main() {
    //enable_debug_log().unwrap();
    let mountpoint = env::args_os().nth(1).unwrap();
    match mount_fs(&PathBuf::from(mountpoint)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
}
