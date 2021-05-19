use alloc::{boxed::Box, string::String, sync::Arc};

use bitflags::*;
use fatfs::ReadWriteSeek;
use hashbrown::HashSet;
use lazy_static::lazy_static;

use super::{
    vnode::{CONSOLE_VNODE, VNODE_HASHSET},
    Vnode, ROOT_DIR,
};
use crate::{
    io::{Error, ErrorKind, Read, Seek, SeekFrom, Write},
    syscall::Errno,
};

bitflags! {
    /// `/usr/include/bits/fcntl-linux.h`
    pub struct OpenFlags: usize {
        /// read only
        const RDONLY    =      0;
        /// write only
        const WRONLY    = 1 << 0;
        /// read write
        const RDWR      = 1 << 1;
        /// create file if it does not exist
        const CREAT    = 1 << 6;
        /// error if CREATE and the file exists
        const EXCL = 1 << 7;
        /// truncate file upon open
        const TRUNC  = 1 << 9;
        /// append on each write
        const APPEND    = 1 << 10;
        /// close on exec
        const CLOEXEC   = 1 << 19;
        ///
        const DIRECTORY = 1 << 21;
    }
}

impl OpenFlags {
    #[inline]
    fn readable(self) -> bool {
        self & (OpenFlags::RDONLY | OpenFlags::RDWR) != OpenFlags::WRONLY
    }

    #[inline]
    fn writable(self) -> bool {
        self & (OpenFlags::WRONLY | OpenFlags::RDWR) != OpenFlags::RDONLY
    }
}

lazy_static! {
    pub static ref STDIN: Arc<FileDescriptor> = Arc::new(FileDescriptor {
        flags: OpenFlags::RDONLY,
        pos: 0,
        vnode: CONSOLE_VNODE.clone(),
    });
    pub static ref STDOUT: Arc<FileDescriptor> = Arc::new(FileDescriptor {
        flags: OpenFlags::WRONLY,
        pos: 0,
        vnode: CONSOLE_VNODE.clone(),
    });
}

pub struct FileDescriptor {
    flags: OpenFlags,
    pos: u64,
    // 多个 fd 可指向同一个 vnode
    pub vnode: Arc<Vnode>,
}

impl Drop for FileDescriptor {
    fn drop(&mut self) {
        if alloc::sync::Arc::<Vnode>::strong_count(&self.vnode) == 2 {
            VNODE_HASHSET.lock(|hs| hs.remove(&self.vnode));
        }
    }
}

impl Read for FileDescriptor {
    fn read(&mut self, buf: &mut [u8]) -> core_io::Result<usize> {
        if self.flags.readable() {
            unsafe { Arc::get_mut_unchecked(&mut self.vnode) }
                .inode
                .seek(SeekFrom::Start(self.pos))
                .unwrap();
            unsafe { Arc::get_mut_unchecked(&mut self.vnode) }
                .inode
                .read(buf)
        } else {
            Err(Error::from(ErrorKind::PermissionDenied))
        }
    }
}
impl Write for FileDescriptor {
    fn write(&mut self, buf: &[u8]) -> core_io::Result<usize> {
        if self.flags.writable() {
            unsafe { Arc::get_mut_unchecked(&mut self.vnode) }
                .inode
                .seek(SeekFrom::Start(self.pos))
                .unwrap();
            unsafe { Arc::get_mut_unchecked(&mut self.vnode) }
                .inode
                .write(buf)
        } else {
            Err(Error::from(ErrorKind::PermissionDenied))
        }
    }

    fn flush(&mut self) -> core_io::Result<()> {
        unsafe { Arc::get_mut_unchecked(&mut self.vnode) }
            .inode
            .flush()
    }
}
impl Seek for FileDescriptor {
    fn seek(&mut self, pos: core_io::SeekFrom) -> core_io::Result<u64> {
        unsafe { Arc::get_mut_unchecked(&mut self.vnode) }
            .inode
            .seek(pos)
    }
}

pub fn file_open(full_path: String, flags: OpenFlags) -> Result<Arc<FileDescriptor>, Errno> {
    // TODO 先转为十分标准的绝对路径，比如连续的 /// 转为 /，..转为父目录
    let mut inode: Box<dyn ReadWriteSeek + Send + Sync> = if flags.contains(OpenFlags::CREAT) {
        Box::new(ROOT_DIR.create_file(full_path.as_str()).unwrap())
    } else if flags.contains(OpenFlags::DIRECTORY) {
        Box::new(ROOT_DIR.open_dir(full_path.as_str()).unwrap())
    } else {
        Box::new(ROOT_DIR.open_file(full_path.as_str()).unwrap())
    };

    let pos = if flags.contains(OpenFlags::APPEND) {
        inode.seek(SeekFrom::End(0)).unwrap()
    } else {
        0
    };

    Ok(Arc::new(FileDescriptor {
        flags,
        pos,
        vnode: Arc::new(Vnode { full_path, inode }),
    }))
}
