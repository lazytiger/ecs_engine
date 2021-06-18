use std::{
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_COMPONENTS: usize = 1024;

lazy_static::lazy_static! {
    pub static ref MODS:Vec<AtomicBool> = {
        let mut mods = Vec::with_capacity(MAX_COMPONENTS);
        for _i in 0..MAX_COMPONENTS {
           mods.push(AtomicBool::new(false));
        }
        mods
    };
}

// mask的最高两位表示修改状态
// 00 表示修改
// 10 表示新建
// 11 表示删除
pub trait Changeset {
    fn mask(&self) -> u128;
    fn mask_mut(&mut self) -> &mut u128;
    fn index() -> usize;

    #[inline]
    fn is_dirty(&self) -> bool {
        self.mask() != 0
    }

    #[inline]
    fn mask_new(&mut self) {
        *self.mask_mut() |= 0x80000000;
    }

    #[inline]
    fn mask_del(&mut self) {
        *self.mask_mut() |= 0xc0000000;
    }

    #[inline]
    fn is_new(&self) -> bool {
        self.mask() & 0x80000000 != 0
    }

    #[inline]
    fn is_del(&self) -> bool {
        self.mask() & 0xc0000000 != 0
    }

    #[inline]
    fn set_storage_dirty() {
        MODS[Self::index()].store(true, Ordering::Relaxed);
    }

    #[inline]
    fn clear_storage_dirty() {
        MODS[Self::index()].store(false, Ordering::Relaxed);
    }

    #[inline]
    fn is_storage_dirty() -> bool {
        MODS[Self::index()].load(Ordering::Relaxed)
    }
}

pub struct ChangeSet<T> {
    data: T,
    mask_db: u64,
    mask_ct: u64,
}

impl<T> Deref for ChangeSet<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for ChangeSet<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> ChangeSet<T>
where
    T: protobuf::Mask,
    T: protobuf::Message,
{
    pub fn new(data: T) -> Self {
        Self {
            data,
            mask_ct: 0,
            mask_db: 0,
        }
    }

    fn commit(&mut self) {
        self.mask_db |= self.mask();
        self.mask_ct |= self.mask();
    }

    pub fn encode_db(&mut self) -> Vec<u8> {
        self.commit();
        let data = self.encode(self.mask_db);
        self.mask_db = 0;
        data
    }

    fn encode(&mut self, mask: u64) -> Vec<u8> {
        *self.data.mask_mut() = mask;
        let data = match self.data.write_to_bytes() {
            Err(err) => {
                log::error!("encode failed {}", err);
                Vec::new()
            }
            Ok(data) => data,
        };
        self.data.clear_mask();
        data
    }

    pub fn encode_ct(&mut self) -> Vec<u8> {
        self.commit();
        let data = self.encode(self.mask_ct);
        self.mask_ct = 0;
        data
    }

    pub fn decode(&mut self, data: &[u8]) {
        if let Err(err) = self.data.merge_from_bytes(data) {
            log::error!("decode failed:{}", err);
        }
    }
}
