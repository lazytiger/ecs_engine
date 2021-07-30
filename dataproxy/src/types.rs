use std::{collections::HashMap, fmt::Write};

use bytes::BytesMut;
use mysql::{prelude::Queryable, FromValueError, PooledConn, Value};

use codegen::FromRow;
use mysql::prelude::{ConvIr, FromValue};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq)]
pub enum BoolValue {
    Yes,
    No,
}

impl Default for BoolValue {
    fn default() -> Self {
        BoolValue::No
    }
}

pub struct BoolValueParser {
    data: Vec<u8>,
}

impl ConvIr<BoolValue> for BoolValueParser {
    fn new(v: Value) -> Result<BoolValueParser, FromValueError> {
        if let Value::Bytes(data) = v {
            Ok(BoolValueParser { data })
        } else {
            Err(FromValueError(v))
        }
    }

    fn commit(self) -> BoolValue {
        if self.data.eq_ignore_ascii_case(&[b'y', b'e', b's']) {
            BoolValue::Yes
        } else {
            BoolValue::No
        }
    }

    fn rollback(self) -> Value {
        Value::Bytes(self.data)
    }
}

impl FromValue for BoolValue {
    type Intermediate = BoolValueParser;
}

#[derive(Debug, FromRow, Clone, Default)]
pub struct Column {
    pub field: String,
    pub field_type: String,
    pub null: BoolValue,
    key: String,
    pub default: Option<String>,
    extra: String,
}

impl Column {
    pub fn to_sql(&self) -> String {
        let sql = format!(
            "`{}` {} {}",
            self.field,
            self.field_type,
            match self.null {
                BoolValue::Yes => "NULL",
                BoolValue::No => "NOT NULL",
            },
        );
        if let Some(default) = &self.default {
            format!("{} DEFAULT '{}'", sql, default)
        } else {
            sql
        }
    }
}

impl PartialEq for Column {
    fn eq(&self, other: &Self) -> bool {
        self.field.eq_ignore_ascii_case(other.field.as_str())
            && self
                .field_type
                .eq_ignore_ascii_case(other.field_type.as_str())
            && self.null == other.null
            && self.default == other.default
    }
}

#[derive(Debug, FromRow, Default)]
pub struct Index {
    table: String,
    pub non_unique: u32,
    pub key_name: String,
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
    visible: BoolValue,
    expression: String,
    clustered: String,
}

impl PartialEq for Index {
    fn eq(&self, other: &Self) -> bool {
        self.table.eq_ignore_ascii_case(&other.table)
            && self.non_unique == other.non_unique
            && self.key_name.eq_ignore_ascii_case(&other.key_name)
            && self.seq_in_index == other.seq_in_index
            && self.column_name.eq_ignore_ascii_case(&other.column_name)
            && self.collation.eq_ignore_ascii_case(&other.collation)
            && self.index_type.eq_ignore_ascii_case(&other.index_type)
    }
}

impl Index {
    pub fn get_collation(&self) -> String {
        if self.collation.eq_ignore_ascii_case("D") {
            "DESC"
        } else {
            "ASC"
        }
        .into()
    }
}

#[derive(Debug, FromRow, Default)]
pub struct TableStatus {
    pub name: String,
    pub engine: String,
    version: u32,
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

#[derive(Debug, Default)]
pub struct Table {
    pub status: TableStatus,
    pub columns: Vec<Column>,
    pub indexes: HashMap<String, Vec<Index>>,
    pub exists: bool,
}

impl Table {
    pub fn new(database: &str, table: &str, conn: &mut PooledConn) -> mysql::Result<Self> {
        if let Some(status) = conn.query_first::<TableStatus, String>(format!(
            "SHOW TABLE STATUS FROM {} like '{}'",
            database, table
        ))? {
            let columns: Vec<Column> =
                conn.query(format!("SHOW COLUMNS FROM {}.{}", database, table))?;
            let indexes: Vec<Index> =
                conn.query(format!("SHOW INDEX FROM {}.{}", database, table))?;

            let mut index_map = HashMap::new();
            for index in indexes {
                if !index_map.contains_key(&index.key_name) {
                    index_map.insert(index.key_name.clone(), Vec::new());
                }
                index_map.get_mut(&index.key_name).unwrap().push(index);
            }
            index_map.iter_mut().for_each(|(_, indexes)| {
                indexes.sort_by(|a, b| a.seq_in_index.cmp(&b.seq_in_index));
            });

            Ok(Self {
                exists: true,
                columns,
                indexes: index_map,
                status,
            })
        } else {
            Ok(Self {
                exists: false,
                columns: Default::default(),
                indexes: Default::default(),
                status: Default::default(),
            })
        }
    }

    pub fn engine(&self) -> String {
        self.status.engine.clone()
    }

    pub fn charset(&self) -> String {
        let sets: Vec<_> = self.status.collation.split("_").collect();
        sets[0].into()
    }

    pub fn collation(&self) -> String {
        self.status.collation.clone()
    }

    pub fn gen_index_sql(name: &String, columns: &Vec<Index>) -> Result<String, std::fmt::Error> {
        let mut buffer = BytesMut::new();
        let index_type = columns[0].index_type.clone();
        let unique = if columns[0].non_unique == 0 {
            "UNIQUE"
        } else {
            ""
        };
        if name.eq_ignore_ascii_case("primary") {
            write!(buffer, "PRIMARY KEY(")?;
        } else {
            write!(buffer, "{} KEY `{}` {} (", unique, name, index_type)?;
        }
        for column in columns {
            write!(
                buffer,
                "`{}` {}, ",
                column.column_name,
                column.get_collation()
            )?;
        }
        buffer.truncate(buffer.len() - 2);
        write!(buffer, ")")?;
        Ok(unsafe { String::from_utf8_unchecked(buffer.to_vec()) })
    }

    fn gen_create_table(&self) -> Result<String, std::fmt::Error> {
        let mut buffer = BytesMut::new();
        writeln!(buffer, "CREATE TABLE `{}`(", self.status.name)?;
        for column in &self.columns {
            writeln!(buffer, "  {},", column.to_sql())?;
        }
        for (index_name, index_col) in &self.indexes {
            writeln!(buffer, "  {},", Self::gen_index_sql(index_name, index_col)?)?;
        }
        buffer.truncate(buffer.len() - 1);
        writeln!(
            buffer,
            ") ENGINE={} DEFAULT CHARSET={} COLLATE={}",
            self.engine(),
            self.charset(),
            self.collation()
        )?;
        return Ok(unsafe { String::from_utf8_unchecked(buffer.to_vec()) });
    }

    fn gen_diff_columns(&self, old: &Self) -> Result<Vec<String>, std::fmt::Error> {
        let mut old_columns: Vec<(usize, &Column)> = old.columns.iter().enumerate().collect();
        old_columns.sort_by(|(_, c1), (_, c2)| c1.field.cmp(&c2.field));

        let mut new_columns: Vec<(usize, &Column)> = self.columns.iter().enumerate().collect();
        new_columns.sort_by(|(_, c1), (_, c2)| c1.field.cmp(&c2.field));

        let mut i = 0;
        let mut j = 0;

        let mut inserted_columns = Vec::new();
        let mut modified_columns = Vec::new();
        let mut removed_columns = Vec::new();
        while i < new_columns.len() && j < old_columns.len() {
            match new_columns[i].1.field.cmp(&old_columns[j].1.field) {
                Ordering::Less => {
                    inserted_columns.push(new_columns[i]);
                    i += 1;
                }
                Ordering::Equal => {
                    if new_columns[i].1 != old_columns[j].1 {
                        modified_columns.push(new_columns[i]);
                    }
                    i += 1;
                    j += 1;
                }
                Ordering::Greater => {
                    removed_columns.push(old_columns[j]);
                    j += 1;
                }
            }
        }

        if i < new_columns.len() {
            inserted_columns.extend_from_slice(&new_columns.as_slice()[i..]);
        }

        if j < old_columns.len() {
            removed_columns.extend_from_slice(&old_columns.as_slice()[j..]);
        }

        let mut result = Vec::new();

        inserted_columns.sort_by(|(index1, _), (index2, _)| index2.cmp(index1));
        for (index, column) in inserted_columns {
            let mut sql = format!(
                "ALTER TABLE `{}` ADD COLUMN {}",
                self.status.name,
                column.to_sql()
            );
            if index != new_columns.len() - 1 {
                sql = format!("{} BEFORE `{}`", sql, self.columns[index + 1].field)
            }
            result.push(sql);
        }

        for (_, column) in modified_columns {
            let sql = format!(
                "ALTER TABLE `{}` MODIFY COLUMN {}",
                self.status.name,
                column.to_sql()
            );
            result.push(sql);
        }

        for (_, column) in removed_columns {
            let sql = format!(
                "ALTER TABLE `{}` DROP COLUMN `{}`",
                self.status.name, column.field
            );
            result.push(sql);
        }

        Ok(result)
    }

    fn gen_diff_indexes(&self, old: &Self) -> Result<Vec<String>, std::fmt::Error> {
        let mut new_indexes: Vec<(&String, &Vec<Index>)> = self.indexes.iter().collect();
        new_indexes.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

        let mut old_indexes: Vec<(&String, &Vec<Index>)> = old.indexes.iter().collect();
        old_indexes.sort_by(|(name1, _), (name2, _)| name1.cmp(name2));

        let mut i = 0;
        let mut j = 0;

        let mut inserted = Vec::new();
        let mut modified = Vec::new();
        let mut removed = Vec::new();

        while i < new_indexes.len() && j < old_indexes.len() {
            match new_indexes[i].0.cmp(old_indexes[i].0) {
                Ordering::Less => {
                    inserted.push(new_indexes[i]);
                    i += 1;
                }
                Ordering::Equal => {
                    if new_indexes[i].1 != old_indexes[j].1 {
                        modified.push(new_indexes[i]);
                    }
                    i += 1;
                    j += 1;
                }
                Ordering::Greater => {
                    removed.push(old_indexes[j]);
                    j += 1;
                }
            }
        }
        if i < new_indexes.len() {
            inserted.extend_from_slice(&new_indexes.as_slice()[i..]);
        }
        if j < old_indexes.len() {
            removed.extend_from_slice(&old_indexes.as_slice()[j..]);
        }

        let mut result = Vec::new();
        for (name, _) in removed {
            result.push(format!(
                "ALTER TABLE `{}` DROP KEY `{}`",
                self.status.name, name
            ));
        }
        for (name, cols) in inserted {
            result.push(format!(
                "ALTER TABLE `{}` ADD {}",
                self.status.name,
                Self::gen_index_sql(name, cols)?
            ));
        }
        for (name, cols) in modified {
            result.push(format!(
                "ALTER TABLE `{}` MODIFY {}",
                self.status.name,
                Self::gen_index_sql(name, cols)?
            ));
        }

        Ok(result)
    }

    fn gen_diff_status(&self, old: &Table) -> Option<String> {
        if self.engine().eq_ignore_ascii_case(&old.engine())
            && self.charset().eq_ignore_ascii_case(&old.charset())
        {
            None
        } else {
            Some(format!(
                "ALTER TABLE `{}` ENGINE=`{}` DEFAULT CHARSET=`{}`",
                self.status.name,
                self.engine(),
                self.charset()
            ))
        }
    }

    pub fn set_charset(&mut self, charset: &str) {
        self.status.collation = format!("{}_bin", charset);
    }

    pub fn set_name(&mut self, name: &str) {
        self.status.name = name.into();
    }

    pub fn set_engine(&mut self, engine: &str) {
        self.status.engine = engine.into();
    }

    pub fn diff(&self, old: &Self) -> Result<Vec<String>, std::fmt::Error> {
        if old.exists {
            let mut columns = self.gen_diff_columns(old)?;
            let indexes = self.gen_diff_indexes(old)?;
            let status = self.gen_diff_status(old);
            columns.extend(indexes.into_iter());
            if let Some(status) = status {
                columns.push(status);
            }
            Ok(columns)
        } else {
            let result = self.gen_create_table()?;
            Ok(vec![result])
        }
    }

    pub fn add_index(&mut self, name: String, columns: &[String], unique: bool, asc: bool) {
        let mut indexes = Vec::new();
        for (i, column) in columns.iter().enumerate() {
            let mut index = Index::default();
            index.key_name = name.clone();
            index.column_name = column.clone();
            index.index_comment = "BTREE".into();
            index.non_unique = if unique { 0 } else { 1 };
            index.visible = BoolValue::Yes;
            index.seq_in_index = i + 1;
            index.collation = if asc { "A" } else { "D" }.into();
            indexes.push(index);
        }
        self.indexes.insert(name, indexes);
    }
}
