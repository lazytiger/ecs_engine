use codegen::FromRow;
use mysql::{prelude::Queryable, PooledConn};
use std::collections::HashMap;

#[derive(Debug, FromRow)]
pub struct Column {
    pub field: String,
    pub field_type: String,
    pub null: String,
    pub key: String,
    pub default: Option<String>,
    pub extra: String,
}

#[derive(Debug, FromRow)]
pub struct Index {
    table: String,
    pub non_unique: u32,
    key_name: String,
    pub seq_in_index: usize,
    pub column_name: String,
    collation: String,
    cardinality: u32,
    sub_part: Option<String>,
    packed: Option<String>,
    null: String,
    pub index_type: String,
    pub comment: String,
    pub index_comment: String,
    visible: String,
    expression: String,
    clustered: String,
}

#[derive(Debug, FromRow)]
pub struct TableStatus {
    pub name: String,
    pub engine: String,
    pub version: u32,
    row_format: String,
    rows: usize,
    avg_row_length: usize,
    data_length: usize,
    max_data_length: usize,
    index_length: usize,
    data_free: usize,
    pub auto_increment: Option<usize>,
    create_time: String,
    update_time: Option<String>,
    check_time: Option<String>,
    collation: String,
    check_sum: String,
    create_options: String,
    pub comment: String,
}

#[derive(Debug)]
pub struct Table {
    status: TableStatus,
    columns: Vec<Column>,
    indexes: HashMap<String, Vec<Index>>,
}

impl Table {
    pub fn new(database: &str, table: &str, conn: &mut PooledConn) -> mysql::Result<Self> {
        let columns: Vec<Column> =
            conn.query(format!("SHOW COLUMNS FROM {}.{}", database, table))?;
        let indexes: Vec<Index> = conn.query(format!("SHOW INDEX FROM {}.{}", database, table))?;
        let status: TableStatus = conn
            .query_first(format!(
                "SHOW TABLE STATUS FROM {} like '{}'",
                database, table
            ))?
            .unwrap();
        let mut index_map = HashMap::new();
        for index in indexes {
            if !index_map.contains_key(&index.key_name) {
                index_map.insert(index.key_name.clone(), Vec::new());
            }
            index_map.get_mut(&index.key_name).unwrap().push(index);
        }

        Ok(Self {
            columns,
            indexes: index_map,
            status,
        })
    }
}
