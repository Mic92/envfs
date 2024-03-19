use concurrent_hashmap::ConcHashMap;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, ReplyStatfs,
    ReplyXattr, Request,
};
use libc::{c_ulong, ENODATA, ENOENT};
use log::debug;
use nix::errno::Errno;
use nix::mount::mount;
use nix::unistd::{self, Pid};
use simple_error::try_with;
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::fs::File;
use std::io::Seek;
use std::io::{BufRead, BufReader};
use std::io::{Read, SeekFrom};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, UNIX_EPOCH};

use crate::result::Result;
use crate::setrlimit::{setrlimit, Rlimit};

const TTL: Duration = Duration::from_secs(1);

const ENVFS_MAGIC: u32 = 0xc7653a76;

const ROOT_DIR_ATTR: FileAttr = FileAttr {
    ino: fuser::FUSE_ROOT_ID,
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
    blksize: 0,
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
    fallback_paths: Arc<Vec<PathBuf>>,
    mountpoints: Vec<PathBuf>,
}

impl EnvFs {
    pub fn new(fallback_paths: &[PathBuf]) -> Result<EnvFs> {
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
            fallback_paths: Arc::new(fallback_paths.to_vec()),
            mountpoints: vec![],
        })
    }

    fn next_inode_number(&self) -> (u64, u64) {
        let mut counter = self.inode_counter.write().unwrap();
        let next_number = counter.next_number;
        counter.next_number += 1;

        if next_number == 0 {
            counter.next_number = fuser::FUSE_ROOT_ID + 1;
            counter.generation += 1;
        }

        (next_number, counter.generation)
    }

    fn inode(&self, ino: u64) -> nix::Result<Arc<Inode>> {
        assert!(ino > 0);

        match self.inodes.find(&ino) {
            Some(inode) => Ok(Arc::clone(inode.get())),
            None => Err(Errno::ESTALE),
        }
    }

    pub fn mount(self, mountpoints: &[PathBuf]) -> Result<fuser::BackgroundSession> {
        assert!(mountpoints.len() > 1);

        let cntrfs = EnvFs {
            inodes: Arc::clone(&self.inodes),
            inode_counter: Arc::clone(&self.inode_counter),
            fallback_paths: Arc::clone(&self.fallback_paths),
            mountpoints: mountpoints.to_vec(),
        };

        let session = try_with!(
            fuser::spawn_mount2(
                cntrfs,
                mountpoints[0].clone(),
                &[
                    fuser::MountOption::FSName("envfs".to_string()),
                    fuser::MountOption::AllowOther,
                    fuser::MountOption::DefaultPermissions,
                    fuser::MountOption::RO
                ]
            ),
            "failed to spawn mount2"
        );

        for mountpoint in mountpoints.iter().skip(1) {
            try_with!(
                fs::create_dir_all(mountpoint),
                "failed to create directory {}",
                mountpoint.display()
            );
            try_with!(
                mount(
                    Some(&mountpoints[0]),
                    mountpoint,
                    None::<&str>,
                    nix::mount::MsFlags::MS_BIND,
                    None::<&str>
                ),
                "failed to bind mount {}",
                mountpoint.display()
            );
        }
        Ok(session)
    }
}

macro_rules! tryfuse {
    ($result:expr, $reply:expr) => {
        match $result {
            Ok(val) => val,
            Err(err) => {
                debug!("return error {} on {}:{}", err, file!(), line!());
                return $reply.error(err as i32);
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
        blksize: 0,
        // Flags (OS X only, see chflags(2))
        flags: 0,
    }
}

fn _which<P1, P2>(path: &Path, exe_name: P1, mountpoints: &[P2]) -> Option<PathBuf>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    if mountpoints.iter().any(|m| m.as_ref().starts_with(path)) {
        return None;
    }

    // Do we still need this check if we already check for mountpoints?
    if let Ok(stat) = path.symlink_metadata() {
        if stat.nlink() as u32 == ENVFS_MAGIC {
            return None;
        }
    }

    let full_path = path.join(&exe_name);
    let res = unistd::access(&full_path, unistd::AccessFlags::X_OK);
    if res.is_ok() {
        Some(full_path)
    } else {
        None
    }
}

fn which<P1, P2>(
    path_env: &OsStr,
    exe_name: P1,
    fallback_paths: &[PathBuf],
    mountpoints: &[P2],
) -> Option<PathBuf>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let exe = env::split_paths(&path_env).find_map(|dir| _which(&dir, &exe_name, mountpoints));

    exe.or_else(|| {
        fallback_paths
            .iter()
            .find_map(|dir| _which(dir, &exe_name, mountpoints))
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

#[cfg(any(
    target_arch = "x86_64",
    target_arch = "arm",
    target_arch = "powerpc",
    target_arch = "powerpc64",
    target_arch = "sparc64",
    target_arch = "mips",
    target_arch = "mips64",
    target_arch = "s390x"
))]
fn is_open_syscall(num: usize) -> bool {
    num == libc::SYS_open as usize || num == libc::SYS_openat as usize
}

#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "arm",
    target_arch = "powerpc",
    target_arch = "powerpc64",
    target_arch = "sparc64",
    target_arch = "mips",
    target_arch = "mips64",
    target_arch = "s390x"
)))]
fn is_open_syscall(num: usize) -> bool {
    num == libc::SYS_openat as usize
}

fn is_execve_syscall(num: usize) -> bool {
    num == libc::SYS_execve as usize || num == libc::SYS_execveat as usize
}

fn resolve_target<P1, P2>(
    pid: Pid,
    name: P1,
    fallback_paths: &[PathBuf],
    mountpoints: &[P2],
) -> Option<PathBuf>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let env = match read_environment(pid) {
        Ok(env) => env,
        Err(_) => {
            return None;
        }
    };
    let args = match get_syscall_args(pid) {
        Ok(args) => args,
        Err(e) => {
            debug!("Could not parse syscall arguments: {}", e);
            return None;
        }
    };
    if args.is_empty() {
        debug!("no syscall arguments received from /proc/<pid>/syscall");
        return None;
    }

    // execve is always allowed and handled differently
    if is_execve_syscall(args[0]) {
        // If we have an execve system call, fetch the latest environment variables from /proc/<pid>/mem
        if args.len() < 4 {
            debug!(
                "expected at least 4 syscall arguments in execve syscall, got {}",
                args.len() - 1
            );
            return None;
        }
        let envp = if args[0] == libc::SYS_execve as usize {
            args[3]
        } else {
            args[4]
        };
        match get_env_from_mem(pid, envp) {
            Ok(env) => {
                if let Some(path) = env.get(OsStr::new("PATH")) {
                    if let Some(exe) = which(path, &name, &[], mountpoints) {
                        return Some(exe);
                    }
                }
            }
            Err(e) => {
                debug!(
                    "Could not read environment variables from child from memory: {}",
                    e
                )
                // fallback to the default path
            }
        }
    }
    let mut path = OsStr::new("");

    // We need to allow open/openat because some programs want to open themself, i.e. bash
    let allowed_syscall = is_open_syscall(args[0])
        || is_execve_syscall(args[0])
        || env.contains_key(OsStr::new("ENVFS_RESOLVE_ALWAYS"));

    if allowed_syscall {
        if let Some(v) = env.get(OsStr::new("PATH")) {
            path = v;
        };
    }

    // We return all paths in fallback path to be resolved always independently
    // of the syscall.
    which(path, &name, fallback_paths, mountpoints)
}

fn get_syscall_args(pid: Pid) -> Result<Vec<usize>> {
    let line = loop {
        let path = format!("/proc/{}/syscall", pid.as_raw());
        let line = try_with!(fs::read_to_string(path), "cannot read syscall file");
        // Sometimes system calls are still in progress when we are trying to read them.
        if line != "running\n" {
            break line;
        }
    };
    let res = line
        .trim_end()
        .split(' ')
        .enumerate()
        .map(|(i, col)| {
            if i == 0 {
                col.parse::<usize>()
            } else {
                usize::from_str_radix(&col[2..], 16)
            }
        })
        .collect::<std::result::Result<Vec<_>, _>>();
    Ok(try_with!(
        res,
        "syscall arguments '{}' cannot be parsed as integer",
        line
    ))
}

fn get_env_from_mem(pid: Pid, envp: usize) -> Result<HashMap<OsString, OsString>> {
    let path = format!("/proc/{}/mem", pid.as_raw());
    let f = try_with!(File::open(&path), "failed to open {}", path);
    let mut reader = BufReader::new(f);
    try_with!(
        reader.seek(SeekFrom::Start(envp as u64)),
        "failed to see in {}",
        &path
    );
    let mut pointer_buf = [0; 8];

    // read content of envp
    let mut env_pointers: Vec<c_ulong> = vec![];
    loop {
        let num = try_with!(reader.read(&mut pointer_buf), "error reading memory");
        if num < 4 {
            break;
        }
        let p = c_ulong::from_ne_bytes(pointer_buf);
        // envp is terminated by a NULL pointer
        if p == 0 {
            break;
        }
        env_pointers.push(p);
    }

    let mut buf = vec![];
    // dereference strings from envp
    let env_vars = env_pointers.iter().map(|p| {
        try_with!(reader.seek(SeekFrom::Start(*p)), "failed to seek to string");
        try_with!(reader.read_until(b'\0', &mut buf), "failed to read string");
        let pair = buf[..buf.len() - 1]
            .splitn(2, |c| *c == b'=')
            .collect::<Vec<_>>();
        let pair = if pair.len() != 2 {
            (OsString::from_vec(pair[0].to_vec()), OsString::new())
        } else {
            (
                OsString::from_vec(pair[0].to_vec()),
                OsString::from_vec(pair[1].to_vec()),
            )
        };
        buf.clear();
        Ok(pair)
    });
    env_vars.collect::<Result<HashMap<_, _>>>()
}

impl Filesystem for EnvFs {
    fn lookup(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        // no subdirectories
        if parent != fuser::FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        let pid = Pid::from_raw(req.pid() as i32);

        match resolve_target(pid, name, self.fallback_paths.as_slice(), &self.mountpoints) {
            Some(path) => {
                let (next_number, generation) = self.next_inode_number();

                let attr = symlink_attr(next_number);

                let inode = Arc::new(Inode {
                    name: PathBuf::from(name),
                    path,
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
        if ino == fuser::FUSE_ROOT_ID {
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
        if ino != fuser::FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        let entries = vec![
            (1, FileType::Directory, "."),
            (1, FileType::Directory, ".."),
        ];

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
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

    fn destroy(&mut self) {
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
            match resolve_target(pid, &inode.name, &self.fallback_paths, &self.mountpoints) {
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
