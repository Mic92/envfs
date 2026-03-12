use concurrent_hashmap::ConcHashMap;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, ReplyStatfs,
    ReplyXattr, Request,
};
use libc::{ENODATA, ENOENT};
use libc::{FILE, endmntent, getmntent, setmntent};
use log::{debug, warn};
use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::mount::mount;
use nix::sys::stat::fstatat;
use nix::unistd::{self, Pid};
use simple_error::try_with;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::{CStr, CString};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::fs::File;
use std::io::Seek;
use std::io::{BufRead, BufReader};
use std::io::{Read, SeekFrom};
use std::mem::size_of;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::OwnedFd;
use std::path::{Component, Path, PathBuf};
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, UNIX_EPOCH};

use crate::result::Result;
use crate::setrlimit::{Rlimit, setrlimit};

const TTL: Duration = Duration::from_secs(1);

const ENVFS_MAGIC: u32 = 0xc7653a76;
const ENVFS_NAME: &str = "envfs";
const ENVFS_NAME_C: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"envfs\0") };

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
    pub nlookup: RwLock<u64>,
}

pub struct EnvFs {
    inodes: Arc<ConcHashMap<u64, Arc<Inode>>>,
    inode_counter: Arc<RwLock<InodeCounter>>,
    fallback_paths: Arc<Vec<PathBuf>>,
    mountpoints: Vec<PathBuf>,
    ready: mpsc::Sender<()>,
}

fn open_mntent(path: &str) -> Result<*mut FILE> {
    let mtab_path = CString::new(path).expect("CString::new failed");
    let mtab_ptr = mtab_path.as_ptr();

    let mtab_file: *mut FILE =
        unsafe { setmntent(mtab_ptr, b"r\0".as_ptr() as *const libc::c_char) };
    if mtab_file.is_null() {
        return Err("Failed to open mtab".into());
    }
    Ok(mtab_file)
}

fn is_envfs_mountpoint(path: &Path) -> Result<bool> {
    let c_path = try_with!(
        CString::new(path.as_os_str().as_bytes()),
        "Failed to convert path to CString"
    );
    let mtab_file = match open_mntent("/etc/mtab") {
        Ok(mtab_file) => mtab_file,
        Err(_) => match open_mntent("/proc/mounts") {
            Ok(mtab_file) => mtab_file,
            Err(_) => return Err("Failed to open mtab".into()),
        },
    };

    let mut mnt: *mut libc::mntent = ptr::null_mut();
    let mut result = false;

    unsafe {
        while !mnt.is_null() {
            mnt = getmntent(mtab_file);
            if !mnt.is_null() {
                let mnt_dir = CStr::from_ptr((*mnt).mnt_dir);
                let fs_name = CStr::from_ptr((*mnt).mnt_fsname);
                if mnt_dir == c_path.as_c_str() && fs_name == ENVFS_NAME_C {
                    result = true;
                    break;
                }
            }
        }
    }

    unsafe { endmntent(mtab_file) };
    Ok(result)
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
            ready: mpsc::channel().0,
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
        assert!(!mountpoints.is_empty());

        let (ready, ready_recv) = mpsc::channel();

        let cntrfs = EnvFs {
            inodes: Arc::clone(&self.inodes),
            inode_counter: Arc::clone(&self.inode_counter),
            fallback_paths: Arc::clone(&self.fallback_paths),
            mountpoints: mountpoints.to_vec(),
            ready,
        };

        let session = try_with!(
            fuser::spawn_mount2(
                cntrfs,
                mountpoints[0].clone(),
                &[
                    fuser::MountOption::FSName(ENVFS_NAME.to_string()),
                    fuser::MountOption::AllowOther,
                    fuser::MountOption::DefaultPermissions,
                    fuser::MountOption::RO
                ]
            ),
            "failed to spawn mount2"
        );

        let _ = ready_recv.recv();

        for mountpoint in mountpoints.iter().skip(1) {
            try_with!(
                fs::create_dir_all(mountpoint),
                "failed to create directory {}",
                mountpoint.display()
            );
            match is_envfs_mountpoint(mountpoint) {
                Ok(true) => {
                    debug!("{} is already a mountpoint", mountpoint.display());
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        "failed to check if {} is a mountpoint: {}",
                        mountpoint.display(),
                        e
                    );
                    continue;
                }
            }
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

/// Open the root directory as an O_PATH fd.
fn open_root() -> Option<OwnedFd> {
    nix::fcntl::open(
        Path::new("/"),
        OFlag::O_PATH | OFlag::O_DIRECTORY,
        nix::sys::stat::Mode::empty(),
    )
    .ok()
}

/// Resolve a path to a real file without triggering recursion into envfs filesystems.
///
/// Uses O_PATH file descriptors with openat()/fstatat()/readlinkat() so that
/// each component is resolved one hop at a time relative to an already-open
/// parent fd. This avoids full path traversals by the kernel, which would
/// deadlock if any component lives on an envfs mountpoint.
///
/// Returns `None` when the path is unresolvable or when resolution would enter
/// an envfs mountpoint.
fn safe_resolve_path<P>(path: &Path, mountpoints: &[P]) -> Option<PathBuf>
where
    P: AsRef<Path>,
{
    const MAX_SYMLINK_DEPTH: usize = 40;

    let mut symlink_count = 0;
    let mut resolved = PathBuf::from("/");
    let mut dir_fd: OwnedFd = open_root()?;
    let mut queue: VecDeque<OsString> = path
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();

    while let Some(component) = queue.pop_front() {
        let comp_path = Path::new(&component);
        match comp_path.components().next()? {
            Component::RootDir => {
                resolved = PathBuf::from("/");
                dir_fd = open_root()?;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
                dir_fd = openat(
                    &dir_fd,
                    Path::new(".."),
                    OFlag::O_PATH | OFlag::O_DIRECTORY,
                    nix::sys::stat::Mode::empty(),
                )
                .ok()?;
            }
            Component::Normal(name) => {
                let candidate = resolved.join(name);

                // Abort if resolution would enter an envfs mountpoint.
                if mountpoints.iter().any(|m| candidate.starts_with(m)) {
                    return None;
                }

                // Stat the component via the parent fd without following
                // symlinks. This never triggers FUSE open/read on the child.
                let stat = fstatat(&dir_fd, Path::new(name), AtFlags::AT_SYMLINK_NOFOLLOW).ok()?;

                // Defense-in-depth: catch envfs inodes even if the
                // mountpoints list is incomplete or stale.
                if stat.st_nlink as u32 == ENVFS_MAGIC {
                    return None;
                }

                if (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK {
                    symlink_count += 1;
                    if symlink_count > MAX_SYMLINK_DEPTH {
                        return None;
                    }
                    let target = nix::fcntl::readlinkat(&dir_fd, Path::new(name)).ok()?;
                    // Prepend the symlink target's components for processing.
                    // `resolved`/`dir_fd` stay as the parent of the symlink,
                    // which is the correct base for relative targets. An
                    // absolute target will begin with Component::RootDir,
                    // resetting both.
                    let target_path = PathBuf::from(target);
                    for c in target_path.components().rev() {
                        queue.push_front(c.as_os_str().to_owned());
                    }
                } else {
                    // Advance into this component.  O_PATH means the kernel
                    // performs a single directory lookup and never sends a FUSE
                    // open/read request.
                    dir_fd = openat(
                        &dir_fd,
                        Path::new(name),
                        OFlag::O_PATH,
                        nix::sys::stat::Mode::empty(),
                    )
                    .ok()?;
                    resolved = candidate;
                }
            }
            Component::Prefix(_) => unreachable!("no Windows prefixes on Linux"),
        }
    }

    Some(resolved)
}

fn _which<P1, P2>(path: &Path, exe_name: P1, mountpoints: &[P2]) -> Option<PathBuf>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let full_path = safe_resolve_path(&path.join(&exe_name), mountpoints)?;
    let res = unistd::access(&full_path, unistd::AccessFlags::X_OK);
    if res.is_ok() { Some(full_path) } else { None }
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

#[cfg(not(target_arch = "aarch64"))]
fn is_access_syscall(num: usize) -> bool {
    num == libc::SYS_access as usize
        || num == libc::SYS_faccessat as usize
        || num == libc::SYS_faccessat2 as usize
}

#[cfg(target_arch = "aarch64")]
fn is_access_syscall(num: usize) -> bool {
    num == libc::SYS_faccessat as usize || num == libc::SYS_faccessat2 as usize
}

// TODO: Currently only supports arch which has the newfstatat system call
fn is_fstatat_syscall(num: usize) -> bool {
    num == libc::SYS_newfstatat as usize
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
            debug!("Could not parse syscall arguments: {e}");
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
        match get_path_from_mem(pid, envp) {
            Ok(path) => {
                if let Some(exe) = which(&path, &name, &[], mountpoints) {
                    return Some(exe);
                }
            }
            Err(e) => {
                debug!("Could not read environment variables from child from memory: {e}")
                // fallback to the default path
            }
        }
    }
    let mut path = OsStr::new("");

    // We need to allow open/openat because some programs want to open themself, i.e. bash
    let allowed_syscall = is_open_syscall(args[0])
        || is_execve_syscall(args[0])
        || is_access_syscall(args[0])
        || is_fstatat_syscall(args[0])
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

fn get_path_from_mem(pid: Pid, envp: usize) -> Result<OsString> {
    let path = format!("/proc/{}/mem", pid.as_raw());
    let f = try_with!(File::open(&path), "failed to open {}", path);
    let mut reader = BufReader::new(f);
    try_with!(
        reader.seek(SeekFrom::Start(envp as u64)),
        "failed to see in {}",
        &path
    );
    let mut pointer_buf = [0; 8];

    // read pointers of envp and dereference it
    let mut env_pointers: Vec<usize> = vec![];
    loop {
        let num = try_with!(reader.read(&mut pointer_buf), "error reading memory");
        if num < size_of::<usize>() {
            break;
        }
        let p = usize::from_ne_bytes(pointer_buf);
        // envp is terminated by a NULL pointer
        if p == 0 {
            break;
        }
        env_pointers.push(p);
    }

    // dereference strings from envp
    let mut buf = vec![];
    assert!(size_of::<usize>() <= size_of::<u64>());
    for p in env_pointers.iter() {
        try_with!(
            reader.seek(SeekFrom::Start(*p as u64)),
            "failed to seek to string"
        );
        try_with!(reader.read_until(b'\0', &mut buf), "failed to read string");
        for var in buf.split(|c| *c == b'\0') {
            if var.starts_with(b"PATH=") {
                return Ok(OsString::from_vec(var[5..].to_vec()));
            }
        }
        buf.clear();
    }
    Ok(OsString::new())
}

impl Filesystem for EnvFs {
    fn init(
        &mut self,
        _req: &Request,
        _config: &mut fuser::KernelConfig,
    ) -> std::result::Result<(), i32> {
        // Fuser spawn_mount may return without filesystem being mounted.
        // Use `init` call to indicate readiness. https://github.com/cberner/fuser/issues/325
        let _ = self.ready.send(());
        Ok(())
    }

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

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs as unix_fs;
    use tempfile::TempDir;

    /// Helper: create a file inside a directory, creating parents as needed.
    fn touch(base: &Path, rel: &str) {
        let p = base.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, "").unwrap();
    }

    #[test]
    fn resolves_plain_path() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a/b/c");

        let mountpoints: Vec<PathBuf> = vec![];
        let resolved = safe_resolve_path(&tmp.path().join("a/b/c"), &mountpoints);
        assert_eq!(resolved, Some(tmp.path().join("a/b/c")));
    }

    #[test]
    fn rejects_path_into_mountpoint() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "usr/bin/sh");

        let mountpoints = vec![tmp.path().join("usr/bin")];
        let resolved = safe_resolve_path(&tmp.path().join("usr/bin/sh"), &mountpoints);
        assert_eq!(resolved, None);
    }

    #[test]
    fn rejects_symlink_pointing_into_mountpoint() {
        // Reproduces the Python venv deadlock from #196: a symlink on PATH
        // points into an envfs mountpoint.
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("usr/bin")).unwrap();
        touch(tmp.path(), "usr/bin/python");
        fs::create_dir_all(tmp.path().join("venv/bin")).unwrap();
        unix_fs::symlink(
            tmp.path().join("usr/bin/python"),
            tmp.path().join("venv/bin/python"),
        )
        .unwrap();

        let mountpoints = vec![tmp.path().join("usr/bin")];
        let resolved = safe_resolve_path(&tmp.path().join("venv/bin/python"), &mountpoints);
        assert_eq!(resolved, None);
    }

    #[test]
    fn rejects_symlink_loop() {
        let tmp = TempDir::new().unwrap();
        unix_fs::symlink(tmp.path().join("b"), tmp.path().join("a")).unwrap();
        unix_fs::symlink(tmp.path().join("a"), tmp.path().join("b")).unwrap();

        let mountpoints: Vec<PathBuf> = vec![];
        let resolved = safe_resolve_path(&tmp.path().join("a"), &mountpoints);
        assert_eq!(resolved, None);
    }

    #[test]
    fn rejects_indirect_symlink_into_mountpoint() {
        // link1 -> link2 -> /mountpoint/exe: chained symlinks that
        // eventually land in an envfs mountpoint.
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("mnt")).unwrap();
        touch(tmp.path(), "mnt/exe");
        unix_fs::symlink(tmp.path().join("mnt/exe"), tmp.path().join("link2")).unwrap();
        unix_fs::symlink(tmp.path().join("link2"), tmp.path().join("link1")).unwrap();

        let mountpoints = vec![tmp.path().join("mnt")];
        let resolved = safe_resolve_path(&tmp.path().join("link1"), &mountpoints);
        assert_eq!(resolved, None);
    }
}
