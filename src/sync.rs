use crate::SyncDirection;

pub trait DataSet: Clone {
    fn commit(&mut self);

    fn encode(&mut self, id: u32, dir: SyncDirection) -> Option<Vec<u8>>;

    fn is_data_dirty(&self) -> bool;

    fn is_direction_enabled(dir: SyncDirection) -> bool;
}
