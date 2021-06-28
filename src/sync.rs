use protobuf::Mask;
use std::{
    collections::HashMap,
    ops::{BitOrAssign, Deref, DerefMut},
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

#[derive(Default)]
pub struct DataSetMask {
    mask: u64,
    children: HashMap<usize, DataSetMask>,
}

impl DataSetMask {
    pub fn clear(&mut self) {
        self.mask = 0;
        for (_, v) in self.children.iter_mut() {
            v.clear();
        }
    }
}

impl BitOrAssign for DataSetMask {
    fn bitor_assign(&mut self, rhs: Self) {
        todo!()
    }
}

pub struct DataSet<T> {
    data: T,
    mask_ct: DataSetMask,
    mask_db: DataSetMask,
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
            mask_ct: Default::default(),
            mask_db: Default::default(),
        }
    }

    fn commit(&mut self) {
        //self.mask_db |= self.mask(); FIXME
        //self.mask_ct |= self.mask(); FIXME
    }

    pub fn encode_db(&mut self) -> Vec<u8> {
        self.commit();
        let data = self.encode(1);
        data
    }

    fn encode(&mut self, typ: u8) -> Vec<u8> {
        //*self.data.mask_mut() = mask; FIXME
        let mask = if typ == 1 {
            &mut self.mask_db
        } else {
            &mut self.mask_ct
        };
        let data = match self.data.write_to_bytes() {
            Err(err) => {
                log::error!("encode failed {}", err);
                Vec::new()
            }
            Ok(data) => data,
        };
        mask.clear();
        self.data.clear_mask();
        data
    }

    pub fn encode_ct(&mut self) -> Vec<u8> {
        self.commit();
        let data = self.encode(0);
        self.mask_ct.clear();
        data
    }

    pub fn decode(&mut self, data: &[u8]) {
        if let Err(err) = self.data.merge_from_bytes(data) {
            log::error!("decode failed:{}", err);
        }
    }
}
