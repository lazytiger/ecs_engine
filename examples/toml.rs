use serde_derive::{Deserialize, Serialize};
use std::{convert::TryInto, fs::File, io::Read};

#[derive(Serialize, Deserialize, Debug)]
pub enum StorageType {
    Vec,
    DefaultVec,
    DenseVec,
    HashMap,
    Null,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ConfigType {
    Request,
    Response,
    Component {
        flagged: bool,
        mask: bool,
        r#type: StorageType,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum DataType {
    String,
    U32,
    U64,
    S32,
    S64,
    F32,
    F64,
    Bool,
    Bytes,
}

impl DataType {
    fn to_rust_type(&self) -> &str {
        match self {
            DataType::String => "string",
            DataType::U32 => "uint32",
            DataType::U64 => "uint64",
            DataType::S32 => "sint32",
            DataType::S64 => "sint64",
            DataType::F32 => "float",
            DataType::F64 => "double",
            DataType::Bool => "bool",
            DataType::Bytes => "bytes",
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Field {
    pub name: String,
    pub r#type: DataType,
    pub field: u32,
    pub repeated: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub name: String,
    pub r#type: Option<ConfigType>,
    pub fields: Vec<Field>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigFile {
    pub configs: Vec<Config>,
}

fn main() {
    toml();
}

fn toml() {
    let file = ConfigFile {
        configs: vec![
            Config {
                name: "UserLogin".to_string(),
                r#type: None,
                fields: vec![
                    Field {
                        name: "username".to_string(),
                        r#type: DataType::String,
                        field: 1,
                        repeated: None,
                    },
                    Field {
                        name: "password".to_string(),
                        r#type: DataType::String,
                        field: 2,
                        repeated: None,
                    },
                ],
            },
            Config {
                name: "LoginResult".to_string(),
                r#type: None,
                fields: vec![Field {
                    name: "result".to_string(),
                    r#type: DataType::S32,
                    field: 1,
                    repeated: None,
                }],
            },
            Config {
                name: "Position".to_string(),
                r#type: Some(ConfigType::Component {
                    flagged: false,
                    mask: false,
                    r#type: StorageType::Vec,
                }),
                fields: vec![
                    Field {
                        name: "x".to_string(),
                        r#type: DataType::F32,
                        field: 1,
                        repeated: None,
                    },
                    Field {
                        name: "".to_string(),
                        r#type: DataType::F32,
                        field: 2,
                        repeated: None,
                    },
                ],
            },
        ],
    };
    println!("{:?}", ron::to_string(&file).unwrap());

    let mut data = File::open("examples/example.ron").unwrap();
    let mut content = String::default();
    data.read_to_string(&mut content).unwrap();
    let file: ConfigFile = ron::from_str(content.as_str()).unwrap();
    println!("{:?}", file);
}
