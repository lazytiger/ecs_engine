use serde_derive::{Deserialize, Serialize};
use std::{convert::TryInto, fs::File, io::Read, marker::PhantomData};

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

pub trait Output {
    fn encode(&self) -> Vec<u8>;
}

pub struct Sender<S> {
    _phantom: PhantomData<S>,
}

impl<S: Output> Sender<S> {
    pub fn send(&self, data: impl Into<S>) {
        todo!()
    }
}

#[derive(derive_more::From)]
pub enum Test {
    Hello(Hello),
}

pub struct Hello;

impl Output for Test {
    fn encode(&self) -> Vec<u8> {
        todo!()
    }
}

pub fn test(sender: &Sender<Test>) {
    sender.send(Hello);
}
