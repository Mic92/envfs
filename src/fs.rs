use cntr_fuse::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, ReplyStatfs,
    ReplyXattr, Request,
};
use concurrent_hashmap::ConcHashMap;
use libc::{c_long, ENODATA, ENOENT};
use log::debug;
use nix::errno::Errno;
use nix::unistd::{self, Pid};
use simple_error::try_with;
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::io::{BufRead, BufReader};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::IntoRawFd;
use std::os::unix::prelude::RawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, UNIX_EPOCH};
use std::{cmp, fs};

use crate::fusefd;
use crate::num_cpus;
use crate::result::Result;
use crate::setrlimit::{setrlimit, Rlimit};

const TTL: Duration = Duration::from_secs(1);

const ENVFS_MAGIC: u32 = 0xc7653a76;

const ROOT_DIR_ATTR: FileAttr = FileAttr {
    ino: cntr_fuse::FUSE_ROOT_ID,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::Directory,
    perm: 0o755,
    nlink: ENVFS_MAGIC,
    uid: 0,
    gid: 0,
    rdev: 0,
    // Flags (OS X only, see chflags(2))
    flags: 0,
};

struct InodeCounter {
    next_number: u64,
    generation: u64,
}

pub struct Inode {
    pub name: PathBuf,
    pub path: PathBuf,
    pub pid: Pid,
    pub kind: FileType,
    pub ino: u64,
    pub nlookup: RwLock<u64>,
}

pub struct EnvFs {
    inodes: Arc<ConcHashMap<u64, Arc<Inode>>>,
    inode_counter: Arc<RwLock<InodeCounter>>,
    fuse_fd: RawFd,
    fallback_paths: Arc<Vec<PathBuf>>,
}

impl EnvFs {
    pub fn new(fallback_paths: &[PathBuf]) -> Result<EnvFs> {
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
            fallback_paths: Arc::new(fallback_paths.to_vec()),
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
                fuse_fd: self.fuse_fd,
                fallback_paths: Arc::clone(&self.fallback_paths),
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

fn _which<P>(path: &PathBuf, exe_name: P) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    let skip_path = match path.symlink_metadata() {
        Ok(stat) => stat.nlink() as u32 == ENVFS_MAGIC,
        Err(_) => true,
    };
    if skip_path {
        return None;
    }

    let full_path = path.join(&exe_name);
    let res = unistd::access(&full_path, unistd::AccessFlags::X_OK);
    if res.is_ok() {
        Some(full_path)
    } else {
        None
    }
}

fn which<P>(path_env: &OsStr, exe_name: P, fallback_paths: &[PathBuf]) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    let exe = env::split_paths(&path_env)
        .filter_map(|dir| _which(&dir, &exe_name))
        .next();
    exe.or_else(|| {
        fallback_paths
            .iter()
            .filter_map(|dir| _which(&dir, &exe_name))
            .next()
    })
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

fn resolve_target<P>(pid: Pid, name: P, fallback_paths: &[PathBuf]) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    let env = match read_environment(pid) {
        Ok(env) => env,
        Err(_) => {
            return None;
        }
    };
    let path = match env.get(OsStr::new("PATH")) {
        Some(v) => v,
        None => {
            return None;
        }
    };
    which(path, &name, fallback_paths)
}

fn get_syscall(pid: Pid) -> Result<c_long> {
    let path = format!("/proc/{}/syscall", pid.as_raw());
    let line = try_with!(fs::read_to_string(path), "cannot read syscall file");
    let fields = line.splitn(2, ' ').collect::<Vec<_>>();
    Ok(try_with!(
        fields[0].parse::<c_long>(),
        "cannot parse syscall number {}",
        fields[0]
    ))
}

impl Filesystem for EnvFs {
    fn lookup(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        // no subdirectories
        if parent != cntr_fuse::FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        let pid = Pid::from_raw(req.pid() as i32);
        let skip_lookup = match get_syscall(pid) {
            Ok(num) => {
                if num != libc::SYS_execve {
                    // Ugly work around for `make`, which does stat on `/bin/sh`
                    // We should fix our nixpkgs make to not do that and rely on `sh`
                    which(&OsStr::new(""), &name, &self.fallback_paths).is_some()
                } else {
                    false
                }
            }
            Err(_) => false,
        };
        if skip_lookup {
            reply.error(ENOENT);
            return;
        }

        match resolve_target(pid, &name, self.fallback_paths.as_slice()) {
            Some(target) => {
                let (next_number, generation) = self.next_inode_number();

                let attr = symlink_attr(next_number);

                let inode = Arc::new(Inode {
                    name: PathBuf::from(name),
                    path: target,
                    pid,
                    kind: attr.kind,
                    ino: attr.ino,
                    nlookup: RwLock::new(1),
                });
                assert!(self.inodes.insert(next_number, inode).is_none());

                reply.entry(&Duration::from_secs(0), &attr, generation);
            }
            None => {
                reply.error(ENOENT);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == cntr_fuse::FUSE_ROOT_ID {
            reply.attr(&TTL, &ROOT_DIR_ATTR);
            return;
        }
        tryfuse!(self.inode(ino), reply);
        reply.attr(&TTL, &symlink_attr(ino));
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        reply.error(ENOENT);
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != cntr_fuse::FUSE_ROOT_ID {
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
    fn getxattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        reply.error(ENODATA);
    }

    fn readlink(&mut self, req: &Request, ino: u64, reply: ReplyData) {
        let inode = tryfuse!(self.inode(ino), reply);
        let pid = Pid::from_raw(req.pid() as i32);
        if inode.pid != pid {
            // unlikely
            match resolve_target(pid, &inode.name, &self.fallback_paths) {
                Some(target) => {
                    reply.data(target.as_os_str().as_bytes());
                    return;
                }
                None => {
                    reply.error(ENOENT);
                    return;
                }
            }
        }
        let data = inode.path.as_os_str().as_bytes();
        reply.data(data);
    }
}
