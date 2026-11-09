//! Полнофункциональный драйвер файловой системы UFS2 с поддержкой Soft Updates Journaling (SUJ).
//!
//! Реализует стандарты BSD UFS2:
//! - Суперблок по смещению 64 КиБ (LBA 128) с магическим числом 0x19540119
//! - Иноды ufs2_dinode (256 байт) с 12 прямыми и 3 косвенными указателями на блоки
//! - Группы цилиндров (struct cg) с битовыми картами фрагментов и инодов
//! - Полный контракт VFS (Inode, FileSystem, POSIX chmod/chown)

pub mod cylinder_group;
pub mod dir;
pub mod inode;
pub mod journal;
pub mod superblock;

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::BlockDevice;
use super::vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};
use self::cylinder_group::{allocate_bit, CylinderGroup};
use self::dir::{find_entry, insert_entry, remove_entry, to_vfs_entries};
use self::inode::{Ufs2Dinode, UFS_NDADDR, UFS_ROOTINO};
use self::journal::UfsJournal;
use self::superblock::{Ufs2Superblock, UFS2_SUPERBLOCK_SECTOR};

const ZERO_BLOCK: [u8; 4096] = [0u8; 4096];

pub struct BitmapSectorCache {
    pub cg_idx: usize,
    pub sec_idx: usize,
    pub cur_sec_lba: u64,
    pub bitmap: [u8; 512],
    pub dirty: bool,
}

pub struct IndirectBlockCache {
    pub sec: u64,
    pub data: [u8; 4096],
}

pub struct UfsInner {
    pub dev: Arc<Mutex<dyn BlockDevice>>,
    pub sb: Mutex<Ufs2Superblock>,
    pub cgs: Mutex<Vec<CylinderGroup>>,
    pub journal: Mutex<Option<UfsJournal>>,
    pub bmap_cache: Mutex<Option<BitmapSectorCache>>,
    pub ib_cache: Mutex<Option<IndirectBlockCache>>,
}

impl UfsInner {
    pub fn read_sectors(&self, sector: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        self.dev.lock().read_sectors(sector, buf).map_err(|_| VfsError::IoError)
    }

    pub fn write_sectors(&self, sector: u64, buf: &[u8]) -> Result<(), VfsError> {
        self.invalidate_indirect_block(sector);
        self.write_metadata_sectors(sector, buf)
    }

    pub fn read_indirect_block(&self, sector: u64, buf: &mut [u8; 4096]) -> Result<(), VfsError> {
        let mut guard = self.ib_cache.lock();
        if let Some(ref c) = *guard {
            if c.sec == sector {
                buf.copy_from_slice(&c.data);
                return Ok(());
            }
        }
        self.read_sectors(sector, buf)?;
        *guard = Some(IndirectBlockCache {
            sec: sector,
            data: *buf,
        });
        Ok(())
    }

    pub fn invalidate_indirect_block(&self, sector: u64) {
        let mut guard = self.ib_cache.lock();
        if let Some(ref c) = *guard {
            if c.sec == sector {
                *guard = None;
            }
        }
    }

    pub fn write_metadata_sectors(&self, sector: u64, buf: &[u8]) -> Result<(), VfsError> {
        let mut guard = self.journal.lock();
        if let Some(journal) = guard.as_mut() {
            journal.commit_transaction(&[(sector, buf)])
        } else {
            self.dev.lock().write_sectors(sector, buf).map_err(|_| VfsError::IoError)
        }
    }

    pub fn write_data_sectors(&self, sector: u64, buf: &[u8]) -> Result<(), VfsError> {
        self.dev.lock().write_sectors(sector, buf).map_err(|_| VfsError::IoError)
    }

    pub fn flush_bmap_cache(&self) -> Result<(), VfsError> {
        let mut guard = self.bmap_cache.lock();
        if let Some(ref mut cache) = *guard {
            if cache.dirty {
                self.write_sectors(cache.cur_sec_lba, &cache.bitmap)?;
                cache.dirty = false;
            }
        }
        Ok(())
    }

    pub fn read_inode(&self, ino: u32) -> Result<Ufs2Dinode, VfsError> {
        if ino == 0 {
            return Err(VfsError::NotFound);
        }
        let sb = *self.sb.lock();
        let cg_idx = ((ino - 1) / (sb.ipg as u32)) as usize;
        let index_in_cg = ((ino - 1) % (sb.ipg as u32)) as usize;

        let cg_start_sec = (cg_idx as u64) * (sb.fpg as u64) * 8;
        let inode_table_sec = cg_start_sec + (sb.iblkno as u64);

        let inode_size = 256;
        let byte_offset = index_in_cg * inode_size;
        let sec_offset = (byte_offset / 512) as u64;
        let offset_in_sec = byte_offset % 512;

        let mut buf = [0u8; 512];
        self.read_sectors(inode_table_sec + sec_offset, &mut buf)?;

        let slice = &buf[offset_in_sec..offset_in_sec + inode_size];
        Ok(Ufs2Dinode::parse(slice))
    }

    pub fn write_inode(&self, ino: u32, inode: &Ufs2Dinode) -> Result<(), VfsError> {
        let sb = *self.sb.lock();
        let cg_idx = ((ino - 1) / (sb.ipg as u32)) as usize;
        let index_in_cg = ((ino - 1) % (sb.ipg as u32)) as usize;

        let cg_start_sec = (cg_idx as u64) * (sb.fpg as u64) * 8;
        let inode_table_sec = cg_start_sec + (sb.iblkno as u64);

        let inode_size = 256;
        let byte_offset = index_in_cg * inode_size;
        let sec_offset = (byte_offset / 512) as u64;
        let offset_in_sec = byte_offset % 512;

        let target_sec = inode_table_sec + sec_offset;
        let mut buf = [0u8; 512];
        self.read_sectors(target_sec, &mut buf)?;

        inode.write_to_slice(&mut buf[offset_in_sec..offset_in_sec + inode_size]);
        self.write_sectors(target_sec, &buf)
    }

    pub fn allocate_block(&self) -> Result<i64, VfsError> {
        let mut cgs = self.cgs.lock();
        let sb = *self.sb.lock();
        let mut cache_guard = self.bmap_cache.lock();

        let (mut cg_idx, mut sec_idx) = match *cache_guard {
            Some(ref cache) => (cache.cg_idx, cache.sec_idx),
            None => (0, 0),
        };

        while cg_idx < cgs.len() {
            let cg = &mut cgs[cg_idx];
            if cg.nfree > 0 {
                let cg_start_sec = (cg_idx as u64) * (sb.fpg as u64) * 8;
                let bitmap_start_sec = cg_start_sec + 2;
                let total_blocks = cg.ndblk as usize;
                let bitmap_sectors = (total_blocks + 4095) / 4096;

                while sec_idx < bitmap_sectors {
                    let cur_sec = bitmap_start_sec + (sec_idx as u64);

                    let need_load = match *cache_guard {
                        Some(ref cache) => cache.cur_sec_lba != cur_sec,
                        None => true,
                    };

                    if need_load {
                        if let Some(ref mut old_cache) = *cache_guard {
                            if old_cache.dirty {
                                self.write_sectors(old_cache.cur_sec_lba, &old_cache.bitmap)?;
                                old_cache.dirty = false;
                            }
                        }

                        let mut buf = [0u8; 512];
                        self.read_sectors(cur_sec, &mut buf)?;
                        *cache_guard = Some(BitmapSectorCache {
                            cg_idx,
                            sec_idx,
                            cur_sec_lba: cur_sec,
                            bitmap: buf,
                            dirty: false,
                        });
                    }

                    let cache = cache_guard.as_mut().unwrap();
                    let remaining_bits = total_blocks.saturating_sub(sec_idx * 4096);
                    let max_bits_in_sec = remaining_bits.min(4096);

                    match allocate_bit(&mut cache.bitmap, max_bits_in_sec) {
                        Ok(local_bit) => {
                            cache.dirty = true;
                            cg.nfree = cg.nfree.saturating_sub(1);
                            let global_bit = (sec_idx * 4096 + local_bit) as u64;
                            let block_lba = cg_start_sec + (global_bit * 8);
                            return Ok(block_lba as i64);
                        }
                        Err(_) => {
                            if cache.dirty {
                                self.write_sectors(cache.cur_sec_lba, &cache.bitmap)?;
                                cache.dirty = false;
                            }
                            sec_idx += 1;
                        }
                    }
                }
            }
            cg_idx += 1;
            sec_idx = 0;
        }
        Err(VfsError::IoError)
    }

    pub fn allocate_inode(&self) -> Result<u32, VfsError> {
        let mut cgs = self.cgs.lock();
        let sb = *self.sb.lock();

        for (cg_idx, cg) in cgs.iter_mut().enumerate() {
            if cg.nifree > 0 {
                let cg_start_sec = (cg_idx as u64) * (sb.fpg as u64) * 8;
                let ibitmap_sec = cg_start_sec + 1; // Битовая карта инодов в секторе 1

                let mut ibitmap = [0u8; 512];
                self.read_sectors(ibitmap_sec, &mut ibitmap)?;

                let bit = allocate_bit(&mut ibitmap, sb.ipg as usize)?;
                cg.nifree = cg.nifree.saturating_sub(1);

                self.write_sectors(ibitmap_sec, &ibitmap)?;

                let ino = (cg_idx as u32) * (sb.ipg as u32) + (bit as u32) + 1;
                return Ok(ino);
            }
        }
        Err(VfsError::IoError)
    }
}

pub struct UfsFileSystem {
    pub inner: Arc<UfsInner>,
    pub root_inode: Arc<UfsInode>,
}

impl UfsFileSystem {
    pub fn open(dev: Arc<Mutex<dyn BlockDevice>>) -> Result<Self, VfsError> {
        let mut sb_buf = vec![0u8; 2048];
        dev.lock()
            .read_sectors(UFS2_SUPERBLOCK_SECTOR, &mut sb_buf)
            .map_err(|_| VfsError::IoError)?;

        let sb = Ufs2Superblock::parse(&sb_buf)?;

        let mut cgs = Vec::with_capacity(sb.ncg as usize);
        for cg_idx in 0..sb.ncg as usize {
            let cg_start_sec = (cg_idx as u64) * (sb.fpg as u64) * 8;
            let mut cg_buf = [0u8; 512];
            if dev.lock().read_sectors(cg_start_sec, &mut cg_buf).is_ok() {
                if let Ok(cg) = CylinderGroup::parse(&cg_buf) {
                    cgs.push(cg);
                }
            }
        }

        let journal = if sb.has_suj() {
            UfsJournal::open(dev.clone(), sb.sujfree as u64).ok()
        } else {
            None
        };

        let inner = Arc::new(UfsInner {
            dev,
            sb: Mutex::new(sb),
            cgs: Mutex::new(cgs),
            journal: Mutex::new(journal),
            bmap_cache: Mutex::new(None),
            ib_cache: Mutex::new(None),
        });

        let root_dinode = inner.read_inode(UFS_ROOTINO)?;
        let root_inode = Arc::new(UfsInode {
            inner: inner.clone(),
            ino: UFS_ROOTINO,
            dinode: Mutex::new(root_dinode),
        });

        Ok(Self { inner, root_inode })
    }
}

impl FileSystem for UfsFileSystem {
    fn root(&self) -> Arc<dyn Inode> {
        self.root_inode.clone()
    }

    fn name(&self) -> &'static str {
        "ufs"
    }
}

pub struct UfsInode {
    inner: Arc<UfsInner>,
    ino: u32,
    dinode: Mutex<Ufs2Dinode>,
}

struct CachedBlock {
    sec: u64,
    data: [u8; 4096],
    dirty: bool,
}

impl CachedBlock {
    fn new() -> Self {
        Self {
            sec: 0,
            data: [0u8; 4096],
            dirty: false,
        }
    }

    fn load_or_use(&mut self, target_sec: u64, inner: &UfsInner) -> Result<(), VfsError> {
        if self.sec != target_sec {
            self.flush(inner)?;
            self.sec = target_sec;
            inner.read_indirect_block(target_sec, &mut self.data)?;
            self.dirty = false;
        }
        Ok(())
    }

    fn init_new(&mut self, target_sec: u64, inner: &UfsInner) -> Result<(), VfsError> {
        self.flush(inner)?;
        self.sec = target_sec;
        self.data.fill(0);
        self.dirty = true;
        Ok(())
    }

    fn flush(&mut self, inner: &UfsInner) -> Result<(), VfsError> {
        if self.dirty && self.sec != 0 {
            inner.write_sectors(self.sec, &self.data)?;
            self.dirty = false;
        }
        Ok(())
    }
}

impl Ufs2Dinode {
    pub fn get_block_sec(&self, logical_block: usize, inner: &UfsInner) -> Option<u64> {
        if logical_block < UFS_NDADDR {
            if self.db[logical_block] != 0 {
                Some(self.db[logical_block] as u64)
            } else {
                None
            }
        } else if logical_block < UFS_NDADDR + 512 {
            let ib_sec = self.ib[0] as u64;
            if ib_sec != 0 {
                let mut ib_buf = [0u8; 4096];
                if inner.read_indirect_block(ib_sec, &mut ib_buf).is_ok() {
                    let idx = logical_block - UFS_NDADDR;
                    let off = idx * 8;
                    let sec = i64::from_le_bytes([
                        ib_buf[off], ib_buf[off + 1], ib_buf[off + 2], ib_buf[off + 3],
                        ib_buf[off + 4], ib_buf[off + 5], ib_buf[off + 6], ib_buf[off + 7],
                    ]);
                    if sec != 0 {
                        Some(sec as u64)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else if logical_block < UFS_NDADDR + 512 + 512 * 512 {
            let dbl_sec = self.ib[1] as u64;
            if dbl_sec != 0 {
                let mut dbl_buf = [0u8; 4096];
                if inner.read_indirect_block(dbl_sec, &mut dbl_buf).is_ok() {
                    let rel = logical_block - (UFS_NDADDR + 512);
                    let lvl1_idx = rel / 512;
                    let lvl2_idx = rel % 512;
                    let off1 = lvl1_idx * 8;
                    let lvl2_sec = i64::from_le_bytes([
                        dbl_buf[off1], dbl_buf[off1 + 1], dbl_buf[off1 + 2], dbl_buf[off1 + 3],
                        dbl_buf[off1 + 4], dbl_buf[off1 + 5], dbl_buf[off1 + 6], dbl_buf[off1 + 7],
                    ]);
                    if lvl2_sec != 0 {
                        let mut lvl2_buf = [0u8; 4096];
                        if inner.read_indirect_block(lvl2_sec as u64, &mut lvl2_buf).is_ok() {
                            let off2 = lvl2_idx * 8;
                            let sec = i64::from_le_bytes([
                                lvl2_buf[off2], lvl2_buf[off2 + 1], lvl2_buf[off2 + 2], lvl2_buf[off2 + 3],
                                lvl2_buf[off2 + 4], lvl2_buf[off2 + 5], lvl2_buf[off2 + 6], lvl2_buf[off2 + 7],
                            ]);
                            if sec != 0 {
                                Some(sec as u64)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl Inode for UfsInode {
    fn metadata(&self) -> Metadata {
        self.dinode.lock().to_metadata(self.ino as u64)
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let dinode = self.dinode.lock();
        if offset >= dinode.size as usize {
            return Ok(0);
        }
        let to_read = buf.len().min((dinode.size as usize) - offset);
        let block_size = 4096;

        let mut read_bytes = 0;
        let mut cur_offset = offset;
        let mut temp_block = [0u8; 4096];

        while read_bytes < to_read {
            let logical_block = cur_offset / block_size;
            let block_internal_offset = cur_offset % block_size;
            let chunk = (block_size - block_internal_offset).min(to_read - read_bytes);

            if let Some(sec) = dinode.get_block_sec(logical_block, &self.inner) {
                self.inner.read_sectors(sec, &mut temp_block)?;
                buf[read_bytes..read_bytes + chunk]
                    .copy_from_slice(&temp_block[block_internal_offset..block_internal_offset + chunk]);
            } else {
                buf[read_bytes..read_bytes + chunk].fill(0);
            }

            read_bytes += chunk;
            cur_offset += chunk;
        }

        Ok(read_bytes)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, VfsError> {
        let mut dinode = self.dinode.lock();
        let block_size = 4096;
        let mut written = 0;
        let mut cur_offset = offset;
        let mut temp_block = [0u8; 4096];

        let mut ib0_cache = CachedBlock::new();
        let mut dbl_lvl1_cache = CachedBlock::new();
        let mut dbl_lvl2_cache = CachedBlock::new();

        while written < buf.len() {
            let logical_block = cur_offset / block_size;
            let block_internal_offset = cur_offset % block_size;
            let chunk = (block_size - block_internal_offset).min(buf.len() - written);

            let mut is_new_block = false;
            let sec = if logical_block < UFS_NDADDR {
                if dinode.db[logical_block] != 0 {
                    dinode.db[logical_block] as u64
                } else {
                    let new_sec = self.inner.allocate_block()? as u64;
                    dinode.db[logical_block] = new_sec as i64;
                    dinode.bytes += 4096;
                    is_new_block = true;
                    new_sec
                }
            } else if logical_block < UFS_NDADDR + 512 {
                let _ib_sec = if dinode.ib[0] != 0 {
                    let s = dinode.ib[0] as u64;
                    ib0_cache.load_or_use(s, &self.inner)?;
                    s
                } else {
                    let new_ib = self.inner.allocate_block()? as u64;
                    dinode.ib[0] = new_ib as i64;
                    dinode.bytes += 4096;
                    ib0_cache.init_new(new_ib, &self.inner)?;
                    new_ib
                };
                let idx = logical_block - UFS_NDADDR;
                let off = idx * 8;
                let cur_entry = i64::from_le_bytes([
                    ib0_cache.data[off], ib0_cache.data[off + 1], ib0_cache.data[off + 2], ib0_cache.data[off + 3],
                    ib0_cache.data[off + 4], ib0_cache.data[off + 5], ib0_cache.data[off + 6], ib0_cache.data[off + 7],
                ]);
                if cur_entry != 0 {
                    cur_entry as u64
                } else {
                    let new_data_sec = self.inner.allocate_block()? as u64;
                    ib0_cache.data[off..off + 8].copy_from_slice(&(new_data_sec as i64).to_le_bytes());
                    ib0_cache.dirty = true;
                    dinode.bytes += 4096;
                    is_new_block = true;
                    new_data_sec
                }
            } else if logical_block < UFS_NDADDR + 512 + 512 * 512 {
                let _dbl_sec = if dinode.ib[1] != 0 {
                    let s = dinode.ib[1] as u64;
                    dbl_lvl1_cache.load_or_use(s, &self.inner)?;
                    s
                } else {
                    let new_dbl = self.inner.allocate_block()? as u64;
                    dinode.ib[1] = new_dbl as i64;
                    dinode.bytes += 4096;
                    dbl_lvl1_cache.init_new(new_dbl, &self.inner)?;
                    new_dbl
                };

                let rel = logical_block - (UFS_NDADDR + 512);
                let lvl1_idx = rel / 512;
                let lvl2_idx = rel % 512;
                let off1 = lvl1_idx * 8;

                let cur_lvl2 = i64::from_le_bytes([
                    dbl_lvl1_cache.data[off1], dbl_lvl1_cache.data[off1 + 1], dbl_lvl1_cache.data[off1 + 2], dbl_lvl1_cache.data[off1 + 3],
                    dbl_lvl1_cache.data[off1 + 4], dbl_lvl1_cache.data[off1 + 5], dbl_lvl1_cache.data[off1 + 6], dbl_lvl1_cache.data[off1 + 7],
                ]);

                let _lvl2_sec = if cur_lvl2 != 0 {
                    let s = cur_lvl2 as u64;
                    dbl_lvl2_cache.load_or_use(s, &self.inner)?;
                    s
                } else {
                    let new_lvl2 = self.inner.allocate_block()? as u64;
                    dbl_lvl1_cache.data[off1..off1 + 8].copy_from_slice(&(new_lvl2 as i64).to_le_bytes());
                    dbl_lvl1_cache.dirty = true;
                    dinode.bytes += 4096;
                    dbl_lvl2_cache.init_new(new_lvl2, &self.inner)?;
                    new_lvl2
                };

                let off2 = lvl2_idx * 8;
                let cur_data = i64::from_le_bytes([
                    dbl_lvl2_cache.data[off2], dbl_lvl2_cache.data[off2 + 1], dbl_lvl2_cache.data[off2 + 2], dbl_lvl2_cache.data[off2 + 3],
                    dbl_lvl2_cache.data[off2 + 4], dbl_lvl2_cache.data[off2 + 5], dbl_lvl2_cache.data[off2 + 6], dbl_lvl2_cache.data[off2 + 7],
                ]);

                if cur_data != 0 {
                    cur_data as u64
                } else {
                    let new_data_sec = self.inner.allocate_block()? as u64;
                    dbl_lvl2_cache.data[off2..off2 + 8].copy_from_slice(&(new_data_sec as i64).to_le_bytes());
                    dbl_lvl2_cache.dirty = true;
                    dinode.bytes += 4096;
                    is_new_block = true;
                    new_data_sec
                }
            } else {
                return Err(VfsError::NotSupported);
            };

            if chunk == block_size && block_internal_offset == 0 {
                self.inner.write_data_sectors(sec, &buf[written..written + chunk])?;
            } else {
                if !is_new_block {
                    self.inner.read_sectors(sec, &mut temp_block)?;
                } else {
                    temp_block.fill(0);
                }
                temp_block[block_internal_offset..block_internal_offset + chunk]
                    .copy_from_slice(&buf[written..written + chunk]);
                self.inner.write_data_sectors(sec, &temp_block)?;
            }

            written += chunk;
            cur_offset += chunk;
        }

        ib0_cache.flush(&self.inner)?;
        dbl_lvl1_cache.flush(&self.inner)?;
        dbl_lvl2_cache.flush(&self.inner)?;
        self.inner.flush_bmap_cache()?;

        if cur_offset as u64 > dinode.size {
            dinode.size = cur_offset as u64;
        }
        self.inner.write_inode(self.ino, &dinode)?;
        Ok(written)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, VfsError> {
        let dinode = self.dinode.lock();
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }

        let total_blocks = ((dinode.size as usize) + 4095) / 4096;
        let mut temp_block = [0u8; 4096];
        for b in 0..total_blocks {
            if let Some(sec) = dinode.get_block_sec(b, &self.inner) {
                self.inner.read_sectors(sec, &mut temp_block)?;
                if let Some(entry) = find_entry(&temp_block, name) {
                    let child_dinode = self.inner.read_inode(entry.ino)?;
                    return Ok(Arc::new(UfsInode {
                        inner: self.inner.clone(),
                        ino: entry.ino,
                        dinode: Mutex::new(child_dinode),
                    }));
                }
            }
        }
        Err(VfsError::NotFound)
    }

    fn create(&self, name: &str, itype: InodeType) -> Result<Arc<dyn Inode>, VfsError> {
        let mut dinode = self.dinode.lock();
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }

        let new_ino = self.inner.allocate_inode()?;
        let (perms, uid, gid) = match itype {
            InodeType::Directory => (0o755, 0, 0),
            _ => (0o644, 0, 0),
        };
        let mut new_dinode = Ufs2Dinode::new_empty(itype, perms, uid, gid);

        if itype == InodeType::Directory {
            let dir_sec = self.inner.allocate_block()? as u64;
            new_dinode.db[0] = dir_sec as i64;
            new_dinode.size = 4096;
            new_dinode.bytes = 4096;

            let mut init_dir = [0u8; 4096];
            dir::init_dir_block(&mut init_dir, new_ino, self.ino);
            self.inner.write_sectors(dir_sec, &init_dir)?;
        }

        self.inner.write_inode(new_ino, &new_dinode)?;

        // Вставляем запись в родительский каталог
        let mut inserted = false;
        let mut temp_block = [0u8; 4096];
        let total_blocks = ((dinode.size as usize) + 4095) / 4096;

        for b in 0..total_blocks {
            if let Some(sec) = dinode.get_block_sec(b, &self.inner) {
                self.inner.read_sectors(sec, &mut temp_block)?;
                if insert_entry(&mut temp_block, new_ino, name, itype).is_ok() {
                    self.inner.write_sectors(sec, &temp_block)?;
                    inserted = true;
                    break;
                }
            }
        }

        if !inserted {
            // Выделяем новый блок под каталог
            let new_sec = self.inner.allocate_block()? as u64;
            let logical_idx = total_blocks;
            if logical_idx < UFS_NDADDR {
                dinode.db[logical_idx] = new_sec as i64;
            } else if logical_idx < UFS_NDADDR + 512 {
                let ib_sec = if dinode.ib[0] != 0 {
                    dinode.ib[0] as u64
                } else {
                    let new_ib = self.inner.allocate_block()? as u64;
                    dinode.ib[0] = new_ib as i64;
                    dinode.bytes += 4096;
                    self.inner.write_sectors(new_ib, &ZERO_BLOCK)?;
                    new_ib
                };
                let mut ib_buf = [0u8; 4096];
                self.inner.read_sectors(ib_sec, &mut ib_buf)?;
                let off = (logical_idx - UFS_NDADDR) * 8;
                ib_buf[off..off + 8].copy_from_slice(&(new_sec as i64).to_le_bytes());
                self.inner.write_sectors(ib_sec, &ib_buf)?;
            }
            dinode.size += 4096;
            dinode.bytes += 4096;

            dir::init_empty_dir_block(&mut temp_block);
            insert_entry(&mut temp_block, new_ino, name, itype)?;
            self.inner.write_sectors(new_sec, &temp_block)?;
            self.inner.write_inode(self.ino, &dinode)?;
        }

        self.inner.flush_bmap_cache()?;

        Ok(Arc::new(UfsInode {
            inner: self.inner.clone(),
            ino: new_ino,
            dinode: Mutex::new(new_dinode),
        }))
    }

    fn unlink(&self, name: &str) -> Result<(), VfsError> {
        let dinode = self.dinode.lock();
        let mut temp_block = [0u8; 4096];
        let total_blocks = ((dinode.size as usize) + 4095) / 4096;

        for b in 0..total_blocks {
            if let Some(sec) = dinode.get_block_sec(b, &self.inner) {
                self.inner.read_sectors(sec, &mut temp_block)?;
                if remove_entry(&mut temp_block, name).is_ok() {
                    self.inner.write_sectors(sec, &temp_block)?;
                    return Ok(());
                }
            }
        }
        Err(VfsError::NotFound)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, VfsError> {
        let dinode = self.dinode.lock();
        if dinode.inode_type() != InodeType::Directory {
            return Err(VfsError::NotADirectory);
        }

        let mut temp_block = [0u8; 4096];
        let mut result = Vec::new();
        let total_blocks = ((dinode.size as usize) + 4095) / 4096;

        for b in 0..total_blocks {
            if let Some(sec) = dinode.get_block_sec(b, &self.inner) {
                self.inner.read_sectors(sec, &mut temp_block)?;
                result.extend(to_vfs_entries(&temp_block));
            }
        }

        Ok(result)
    }

    fn chmod(&self, mode: u16) -> Result<(), VfsError> {
        let mut dinode = self.dinode.lock();
        dinode.mode = (dinode.mode & 0o170000) | (mode & 0o7777);
        self.inner.write_inode(self.ino, &dinode)
    }

    fn chown(&self, uid: u32, gid: u32) -> Result<(), VfsError> {
        let mut dinode = self.dinode.lock();
        dinode.uid = uid;
        dinode.gid = gid;
        self.inner.write_inode(self.ino, &dinode)
    }
}
