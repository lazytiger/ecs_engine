mod dataset;
mod generator;
mod request;
mod response;

use std::{
    fmt::Write as _,
    fs::{read_dir, File},
    io::{Read, Write},
    path::PathBuf,
    process::Command,
};

use byteorder::{BigEndian, ByteOrder};
use derive_more::From;
use proc_macro2::TokenStream;
use protobuf_codegen_pure::{Codegen, Customize};
use quote::quote;
use serde_derive::{Deserialize, Serialize};

use bytes::BytesMut;
pub use generator::Generator;
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialOrd, PartialEq)]
pub enum SyncDirection {
    Around,
    Client,
    Database,
    Team,
}

impl From<usize> for SyncDirection {
    fn from(index: usize) -> Self {
        match index {
            1 => SyncDirection::Around,
            2 => SyncDirection::Client,
            4 => SyncDirection::Database,
            8 => SyncDirection::Team,
            _ => panic!("invalid index:{}", index),
        }
    }
}

impl Into<usize> for SyncDirection {
    fn into(self) -> usize {
        match self {
            SyncDirection::Around => 1,
            SyncDirection::Client => 2,
            SyncDirection::Database => 4,
            SyncDirection::Team => 8,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum StorageType {
    Vec,
    DenseVec,
    HashMap,
    DefaultVec,
    Null,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Trait {
    Component {
        storage: StorageType,
        flagged: Option<bool>,
    },
    Position {
        x: Option<String>,
        y: Option<String>,
    },
    SceneData {
        id: Option<String>,
        min_x: Option<String>,
        min_y: Option<String>,
        column: Option<String>,
        row: Option<String>,
        grid_size: Option<String>,
    },
    DropEntity {
        entities: Option<String>,
    },
}

impl Trait {
    pub fn to_rust_type(&self) -> TokenStream {
        if let Trait::Component { storage, flagged } = self {
            let flagged = flagged.is_some() && flagged.unwrap();
            let rust_type = match storage {
                StorageType::Vec => quote!(VecStorage<Self>),
                StorageType::HashMap => quote!(HashMapStorage<Self>),
                StorageType::DenseVec => quote!(DenseVecStorage<Self>),
                StorageType::Null => quote!(NullStorage<Self>),
                StorageType::DefaultVec => quote!(DefaultVecStorage<Self>),
            };
            if flagged {
                quote!(FlaggedStorage<Self, #rust_type>)
            } else {
                rust_type
            }
        } else {
            quote!()
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum DataType {
    String {
        size: Option<usize>,
    },
    U32 {
        size: Option<usize>,
    },
    U64,
    S32 {
        size: Option<usize>,
    },
    S64,
    F32,
    F64,
    Bool,
    Bytes {
        size: Option<usize>,
    },
    List {
        r#type: Box<DataType>,
        size: Option<usize>,
    },
    Map {
        key: Box<DataType>,
        value: Box<DataType>,
        size: Option<usize>,
    },
    Custom {
        r#type: String,
        size: Option<usize>,
    },
}

impl DataType {
    fn to_pb_type(&self) -> String {
        match self {
            DataType::String { .. } => "string".into(),
            DataType::U32 { .. } => "uint32".into(),
            DataType::U64 => "uint64".into(),
            DataType::S32 { .. } => "sint32".into(),
            DataType::S64 => "sint64".into(),
            DataType::F32 => "float".into(),
            DataType::F64 => "double".into(),
            DataType::Bool => "bool".into(),
            DataType::Bytes { .. } => "bytes".into(),
            DataType::Custom { r#type, .. } => r#type.clone(),
            DataType::List { r#type, .. } => format!("repeated {}", r#type.to_pb_type()),
            DataType::Map { key, value, .. } => {
                format!("map<{}, {}>", key.to_pb_type(), value.to_pb_type())
            }
        }
    }

    fn db_integer_type(len: usize) -> String {
        if len <= 3 {
            "TINYINT(3)"
        } else if len <= 5 {
            "SMALLINT(5)"
        } else if len <= 9 {
            "MEDIUMINT(9)"
        } else {
            "INT(11)"
        }
        .into()
    }

    fn db_bytes_type(len: usize) -> String {
        if len <= 1 << 8 {
            "TINYBLOB"
        } else if len <= 1 << 16 {
            "BLOB"
        } else if len <= 1 << 24 {
            "MEDIUMBLOB"
        } else {
            "LONGBLOB"
        }
        .into()
    }

    fn to_db_type(&self) -> String {
        match self {
            DataType::String { size: Some(len) } => format!("VARCHAR({})", len),
            DataType::U32 { size: Some(len) } => {
                format!("{} UNSIGNED", Self::db_integer_type(*len))
            }
            DataType::U64 => "BIGINT(20) UNSIGNED".into(),
            DataType::S32 { size: Some(len) } => Self::db_integer_type(*len),
            DataType::S64 => "BIGINT(20) UNSIGNED".into(),
            DataType::F32 => "FLOAT".into(),
            DataType::F64 => "DOUBLE".into(),
            DataType::Bool => "TINYINT(3) UNSIGNED".into(),
            DataType::Bytes { size } => Self::db_bytes_type(size.unwrap_or(1 << 16)),
            DataType::List { size, .. } => Self::db_bytes_type(size.unwrap_or(1 << 16)),
            DataType::Map { size, .. } => Self::db_bytes_type(size.unwrap_or(1 << 16)),
            DataType::Custom { size, .. } => Self::db_bytes_type(size.unwrap_or(1 << 16)),
            _ => {
                panic!("database type should specify length")
            }
        }
    }

    fn to_rust_type(&self) -> TokenStream {
        match self {
            DataType::String { .. } => quote!(String),
            DataType::U32 { .. } => quote!(u32),
            DataType::U64 => quote!(u64),
            DataType::S32 { .. } => quote!(i32),
            DataType::S64 => quote!(i64),
            DataType::F32 => quote!(f32),
            DataType::F64 => quote!(f64),
            DataType::Bool => quote!(bool),
            DataType::Bytes { .. } => quote!(Vec<u8>),
            DataType::List { .. } => quote!(Vec<u8>),
            DataType::Map { .. } => quote!(Vec<u8>),
            DataType::Custom { .. } => quote!(Vec<u8>),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Field {
    pub name: String,
    pub r#type: DataType,
    pub index: u32,
    pub dirs: Option<Vec<SyncDirection>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub name: String,
    pub hide: Option<bool>,
    pub traits: Option<Vec<Trait>>,
    pub indexes: Option<HashMap<IndexType, TableIndex>>,
    pub fields: Vec<Field>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigFile {
    pub configs: Vec<Config>,
}

impl Config {
    fn max_number(&self) -> u32 {
        if let Some(c) = self.fields.iter().max_by(|a, b| a.index.cmp(&b.index)) {
            c.index
        } else {
            0
        }
    }

    fn get_dir_mask(&self) -> usize {
        let mut mask = 0usize;
        for f in &self.fields {
            if let Some(dirs) = &f.dirs {
                for dir in dirs {
                    let dir: usize = (*dir).into();
                    mask |= dir;
                }
            } else {
                return 0x0f;
            }
        }
        mask
    }

    fn is_database_column(&self, column: &str) -> bool {
        self.fields
            .iter()
            .filter(|field| {
                field.dirs.is_none()
                    || field
                        .dirs
                        .as_ref()
                        .unwrap()
                        .contains(&SyncDirection::Database)
            })
            .any(|field| field.name.eq_ignore_ascii_case(column))
    }

    fn get_primary_cond(&self) -> Result<String, std::fmt::Error> {
        if let Some(indexes) = &self.indexes {
            if let Some(index) = indexes.get(&IndexType::Primary) {
                let mut buffer = BytesMut::new();
                for column in &index.columns {
                    write!(buffer, "`{}` = ? AND ", column)?;
                }
                buffer.truncate(buffer.len() - 5);
                return Ok(unsafe { String::from_utf8_unchecked(buffer.to_vec()) });
            }
        }
        panic!("primary key not found in {}", self.name);
    }

    fn get_primary_fields(&self) -> Vec<String> {
        if let Some(indexes) = &self.indexes {
            if let Some(index) = indexes.get(&IndexType::Primary) {
                return index.columns.clone();
            }
        }
        panic!("primary key not found in {}", self.name);
    }
}

#[derive(PartialOrd, PartialEq, Serialize, Deserialize, Debug, Eq, Hash)]
pub enum IndexType {
    Primary,
    Index(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TableIndex {
    columns: Vec<String>,
    asc: Option<bool>,
    unique: Option<bool>,
}

#[derive(Debug, From)]
pub enum Error {
    Io(std::io::Error),
    Ron(ron::Error, PathBuf),
    Fmt(std::fmt::Error),
    DuplicateFieldNumber(String),
    DuplicateCmd,
    DuplicateDropEntity,
    DuplicatePosition,
    DuplicateSceneData,
    InvalidDropEntity,
    DuplicateIndexColumn,
    InvalidIndexColumnName,
    ComponentListUsed(PathBuf, String),
}

pub fn read_files(input_dir: PathBuf) -> std::io::Result<Vec<PathBuf>> {
    let mut inputs = Vec::new();
    for f in read_dir(input_dir)? {
        let f = f?;
        if f.file_type()?.is_file() {
            inputs.push(f.path());
        }
    }
    Ok(inputs)
}

pub fn parse_config(config_dir: PathBuf) -> Result<Vec<(PathBuf, ConfigFile)>, Error> {
    let files = read_files(config_dir)?;
    let mut configs = Vec::new();
    for input in files {
        let mut file = File::open(&input)?;
        let mut data = String::new();
        file.read_to_string(&mut data)?;
        let cf = match ron::from_str::<ConfigFile>(data.as_str()) {
            Err(err) => {
                return Err(Error::from((err, input.clone())));
            }
            Ok(cf) => cf,
        };
        if let Some(config) = cf.configs.iter().find(|config| {
            let mut fields: Vec<_> = config.fields.iter().map(|f| f.index).collect();
            let count = fields.len();
            fields.sort();
            fields.dedup();
            count != fields.len()
        }) {
            return Err(Error::DuplicateFieldNumber(format!(
                "{:?} - {}",
                input, config.name
            )));
        }
        configs.push((input.clone(), cf));
    }
    Ok(configs)
}

pub fn gen_protos(input_dir: PathBuf, output_dir: PathBuf) -> std::io::Result<()> {
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir.clone())?;
    }
    let files = read_files(input_dir.clone())?;
    let mut customize = Customize::default();
    customize.generate_accessors = Some(true);
    customize.expose_fields = Some(false);
    let mut codegen = Codegen::new();
    codegen
        .customize(customize)
        .inputs(files.iter())
        .include(input_dir)
        .out_dir(output_dir)
        .run()
}

pub fn string_to_u32(name: &[u8]) -> u32 {
    let digest = md5::compute(name).0;
    BigEndian::read_u32(&digest[..4])
}

pub fn gen_messages(
    configs: &Vec<(PathBuf, ConfigFile)>,
    output_dir: PathBuf,
    mask: bool,
) -> Result<(), Error> {
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir.clone())?;
    }
    for (k, v) in configs {
        let mut name = k.file_stem().unwrap().to_owned();
        name.push(".proto");
        let mut path = output_dir.clone();
        path.push(name);
        let mut file = File::create(path)?;
        gen_message(&mut file, &v, mask)?;
    }
    Ok(())
}

pub fn format_file(file: PathBuf) -> std::io::Result<()> {
    Command::new("rustfmt").arg(file).output()?;
    Ok(())
}

/// 根据Config类型生成一个Protobuf配置文件
pub fn gen_message(file: &mut File, cf: &ConfigFile, mask: bool) -> std::io::Result<()> {
    writeln!(file, r#"syntax = "proto3";"#)?;
    for v in &cf.configs {
        writeln!(file, "message {} {{", v.name)?;
        for field in &v.fields {
            writeln!(
                file,
                "\t{} {} = {};",
                field.r#type.to_pb_type(),
                field.name,
                field.index,
            )?;
        }
        if mask {
            writeln!(file, "\tuint64 _mask = {};", v.max_number() + 1)?;
            writeln!(file, "\tbool _deleted = {};", v.max_number() + 2)?;
        }
        writeln!(file, "}}")?;
    }
    Ok(())
}
