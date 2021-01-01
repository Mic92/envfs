use simple_error::try_with;
use std::env;
use std::path::{Path, PathBuf};
use std::process::exit;
use nix::mount;
use nix::unistd;

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

fn mount_fs(mountpoint: &Path) -> Result<()> {
    let fs = try_with!(EnvFs::new(), "cannot create filesystem");
    try_with!(fs.mount(mountpoint), "cannot mount filesystem");

    let guard = MountGuard {
        mount_point: mountpoint,
    };
    let sessions = fs.spawn_sessions().unwrap();
    unistd::pause() ;
    drop(guard);
    drop(sessions);

    Ok(())
}

impl<'a> Drop for MountGuard<'a> {
    fn drop(&mut self) {
        let _ = mount::umount(self.mount_point);
    }
}

fn main() {
    enable_debug_log().unwrap();
    let mountpoint = env::args_os().nth(1).unwrap();
    match mount_fs(&PathBuf::from(mountpoint)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
}
