use crate::SyncDirection;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_COMPONENTS: usize = 1024;

lazy_static::lazy_static! {
    static ref MODS:Vec<AtomicBool> = {
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

pub trait DataSet {
    fn commit(&mut self);

    fn encode(&mut self, dir: SyncDirection) -> Vec<u8>;

    fn is_dirty(&self) -> bool;
}
