use cntr_fuse::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use concurrent_hashmap::ConcHashMap;
use libc::ENOENT;
use log::debug;
use nix::errno::Errno;
use nix::unistd::{self, Pid};
use simple_error::try_with;
use std::cmp;
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::io::{BufRead, BufReader};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::IntoRawFd;
use std::os::unix::prelude::RawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, UNIX_EPOCH};

use crate::fusefd;
use crate::num_cpus;
use crate::result::Result;
use crate::setrlimit::{setrlimit, Rlimit};

// 1 second
const TTL: Duration = Duration::from_secs(1);

const ROOT_DIR_ATTR: FileAttr = FileAttr {
    ino: cntr_fuse::FUSE_ROOT_ID,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    uid: 0,
    gid: 0,
    perm: 0o755,
    kind: FileType::Directory,
    nlink: 2,
    rdev: 0,
    // Flags (OS X only, see chflags(2))
    flags: 0,
};

struct InodeCounter {
    next_number: u64,
    generation: u64,
}

pub struct Inode {
    pub name: std::ffi::OsString,
    pub kind: FileType,
    pub ino: u64,
    pub nlookup: RwLock<u64>,
}

pub struct EnvFs {
    inodes: Arc<ConcHashMap<u64, Arc<Inode>>>,
    inode_counter: Arc<RwLock<InodeCounter>>,
    fuse_fd: RawFd,
}

impl EnvFs {
    pub fn new() -> Result<EnvFs> {
        let fuse_fd = try_with!(fusefd::open(), "failed to initialize fuse");

        let limit = Rlimit {
            rlim_cur: 1_048_576,
            rlim_max: 1_048_576,
        };
        try_with!(
            setrlimit(libc::RLIMIT_NOFILE, &limit),
            "Cannot raise file descriptor limit"
        );

        Ok(EnvFs {
            inodes: Arc::new(ConcHashMap::<u64, Arc<Inode>>::new()),
            inode_counter: Arc::new(RwLock::new(InodeCounter {
                next_number: 3,
                generation: 0,
            })),
            fuse_fd: fuse_fd.into_raw_fd(),
        })
    }

    pub fn mount(&self, mountpoint: &Path) -> Result<()> {
        let mount_flags = format!(
            "fd={},rootmode=40000,user_id=0,group_id=0,allow_other,default_permissions",
            self.fuse_fd
        );

        const NONE: Option<&'static [u8]> = None;
        try_with!(
            nix::mount::mount(
                NONE,
                mountpoint,
                Some("fuse.envfs"),
                nix::mount::MsFlags::empty(),
                Some(mount_flags.as_str()),
            ),
            "failed to mount fuse"
        );
        Ok(())
    }

    fn next_inode_number(&self) -> (u64, u64) {
        let mut counter = self.inode_counter.write().unwrap();
        let next_number = counter.next_number;
        counter.next_number += 1;

        if next_number == 0 {
            counter.next_number = cntr_fuse::FUSE_ROOT_ID + 1;
            counter.generation += 1;
        }

        (next_number, counter.generation)
    }

    fn inode(&self, ino: u64) -> nix::Result<Arc<Inode>> {
        assert!(ino > 0);

        match self.inodes.find(&ino) {
            Some(inode) => Ok(Arc::clone(inode.get())),
            None => Err(nix::Error::Sys(Errno::ESTALE)),
        }
    }

    pub fn spawn_sessions(self) -> Result<Vec<JoinHandle<io::Result<()>>>> {
        let mut sessions = Vec::new();

        // numbers of sessions is optimized for cached read
        let num_sessions = cmp::max(num_cpus::get() / 2, 1) as usize;

        for _ in 0..num_sessions {
            debug!("spawn worker");

            let cntrfs = EnvFs {
                inodes: Arc::clone(&self.inodes),
                inode_counter: Arc::clone(&self.inode_counter),
                fuse_fd: self.fuse_fd.into_raw_fd(),
            };

            let max_background = num_sessions as u16;
            let res = cntr_fuse::Session::new_from_fd(
                cntrfs,
                self.fuse_fd,
                Path::new(""),
                max_background,
                max_background,
            );
            let session = try_with!(res, "failed to inherit fuse session");

            let guard = thread::spawn(move || {
                let mut se = session;
                se.run()
            });

            sessions.push(guard);
        }

        Ok(sessions)
    }
}

macro_rules! tryfuse {
    ($result:expr, $reply:expr) => {
        match $result {
            Ok(val) => val,
            Err(err) => {
                debug!("return error {} on {}:{}", err, file!(), line!());
                let rc = match err {
                    nix::Error::Sys(errno) => errno as i32,
                    // InvalidPath, InvalidUtf8, UnsupportedOperation
                    _ => libc::EINVAL,
                };
                return $reply.error(rc);
            }
        }
    };
}

fn symlink_attr(ino: u64) -> FileAttr {
    FileAttr {
        ino,
        size: 0,
        blocks: 0,
        atime: UNIX_EPOCH,
        mtime: UNIX_EPOCH,
        ctime: UNIX_EPOCH,
        crtime: UNIX_EPOCH,
        uid: 0,
        gid: 0,
        perm: 777,
        kind: FileType::Symlink,
        nlink: 1,
        rdev: 0,
        // Flags (OS X only, see chflags(2))
        flags: 0,
    }
}

pub fn which<P>(path_env: &OsStr, exe_name: P) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    env::split_paths(&path_env)
        .filter_map(|dir| {
            let full_path = dir.join(&exe_name);
            let res = unistd::access(&full_path, unistd::AccessFlags::X_OK);
            if res.is_ok() {
                Some(full_path)
            } else {
                None
            }
        })
        .next()
}

fn read_environment(pid: unistd::Pid) -> Result<HashMap<OsString, OsString>> {
    let path = PathBuf::from("/proc").join(pid.to_string()).join("environ");
    let f = try_with!(File::open(&path), "failed to open {}", path.display());
    let reader = BufReader::new(f);
    let res: HashMap<OsString, OsString> = reader
        .split(b'\0')
        .filter_map(|var| {
            let var = match var {
                Ok(var) => var,
                Err(_) => return None,
            };

            let tuple: Vec<&[u8]> = var.splitn(2, |b| *b == b'=').collect();
            if tuple.len() != 2 {
                return None;
            }
            Some((
                OsString::from_vec(Vec::from(tuple[0])),
                OsString::from_vec(Vec::from(tuple[1])),
            ))
        })
        .collect();
    Ok(res)
}

impl Filesystem for EnvFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        // no subdirectories
        if parent != cntr_fuse::FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        let (next_number, generation) = self.next_inode_number();

        let attr = symlink_attr(next_number);

        let inode = Arc::new(Inode {
            name: OsString::from(name),
            kind: attr.kind,
            ino: attr.ino,
            nlookup: RwLock::new(1),
        });
        assert!(self.inodes.insert(next_number, inode).is_none());

        reply.entry(&TTL, &attr, generation);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == cntr_fuse::FUSE_ROOT_ID {
            reply.attr(&TTL, &ROOT_DIR_ATTR);
            return;
        }
        tryfuse!(self.inode(ino), reply);
        reply.attr(&TTL, &symlink_attr(ino));
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 1 {
            reply.error(ENOENT);
            return;
        }

        let entries = vec![
            (1, FileType::Directory, "."),
            (1, FileType::Directory, ".."),
        ];

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            reply.add(entry.0, (i + 1) as i64, entry.1, entry.2);
        }
        reply.ok();
    }

    fn forget(&mut self, _req: &Request, ino: u64, nlookup: u64) {
        match self.inodes.find_mut(&ino) {
            Some(ref mut inode_lock) => {
                let inode = inode_lock.get();
                let mut old_nlookup = inode.nlookup.write().unwrap();
                assert!(*old_nlookup >= nlookup);

                *old_nlookup -= nlookup;

                if *old_nlookup != 0 {
                    return;
                };
            }
            None => return,
        };

        self.inodes.remove(&ino);
    }

    fn destroy(&mut self, _req: &Request) {
        self.inodes.clear();
    }

    fn readlink(&mut self, req: &Request, ino: u64, reply: ReplyData) {
        let inode = tryfuse!(self.inode(ino), reply);
        let env = match read_environment(Pid::from_raw(req.pid() as i32)) {
            Ok(env) => env,
            Err(_) => {
                reply.error(ENOENT);
                return;
            }
        };
        let path = match env.get(OsStr::new("PATH")) {
            Some(v) => v,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match which(path, &inode.name) {
            Some(target) => {
                let data = target.as_os_str().as_bytes();
                reply.data(data);
            }
            None => {
                reply.error(ENOENT);
            }
        }
    }
}
