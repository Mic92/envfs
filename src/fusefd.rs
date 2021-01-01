use nix::fcntl::OFlag;
use nix::sys::stat::{makedev, mknod, Mode, SFlag};
use nix::{self, errno, fcntl};
use simple_error::{try_with, SimpleError};
use std::fs::File;
use std::os::unix::prelude::*;

use crate::result::Result;

pub fn open() -> Result<File> {
    let res = fcntl::open("/dev/fuse", OFlag::O_RDWR, Mode::empty());

    match res {
        Ok(fd) => {
            let file = unsafe { File::from_raw_fd(fd) };
            return Ok(file);
        }

        Err(nix::Error::Sys(errno::Errno::ENOENT)) => {}
        Err(err) => return Err(SimpleError::with("failed to open /dev/fuse", err)),
    };

    try_with!(
        mknod(
            "/dev/fuse",
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IWUSR,
            makedev(10, 229),
        ),
        "failed to create temporary fuse character device"
    );

    let file = unsafe {
        File::from_raw_fd(try_with!(
            fcntl::open("/dev/fuse", OFlag::O_RDWR, Mode::empty()),
            "failed to open fuse device"
        ))
    };
    Ok(file)
}
