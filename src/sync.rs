use protobuf::Mask;
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

pub trait ChangeSet {
    fn index() -> usize;

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

pub struct DataSet<T> {
    data: T,
    mask_db: u64,
    mask_ct: u64,
}

impl<T> Deref for DataSet<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for DataSet<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> DataSet<T>
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
