use crate::SyncDirection;

pub trait DataSet: Clone {
    fn commit(&mut self);

    fn encode(&mut self, id: u32, dir: SyncDirection) -> Option<Vec<u8>>;

    fn is_data_dirty(&self) -> bool;

    fn is_direction_enabled(dir: SyncDirection) -> bool;
}

pub trait DataBackend {
    type Connection;
    type Error;

    fn patch_table(
        conn: &mut Self::Connection,
        exec: bool,
        database: Option<&str>,
    ) -> Result<Vec<String>, Self::Error>;

    fn select(&mut self, conn: &mut Self::Connection) -> Result<bool, Self::Error>;

    fn insert(&mut self, conn: &mut Self::Connection) -> Result<bool, Self::Error>;

    fn update(&mut self, conn: &mut Self::Connection) -> Result<bool, Self::Error>;

    fn delete(self, conn: &mut Self::Connection) -> Result<bool, Self::Error>;
}
