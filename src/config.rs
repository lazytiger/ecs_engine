use std::{
    fs::{read_dir, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use crate::component::VecComponent;
use byteorder::{BigEndian, ByteOrder};
use derive_more::From;
use proc_macro2::{Ident, TokenStream};
use protobuf_codegen_pure::{Codegen, Customize};
use quote::{format_ident, quote};
use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum StorageType {
    Vec,
    DenseVec,
    HashMap,
    DefaultVec,
    Null,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Component {
    pub storage: StorageType,
    pub flagged: Option<bool>,
}

impl Component {
    pub fn to_rust_type(&self) -> TokenStream {
        let flagged = self.flagged.is_some() && self.flagged.unwrap();
        let rust_type = match self.storage {
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
    }
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
    fn to_pb_type(&self) -> &str {
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
    pub index: u32,
    pub repeated: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub name: String,
    pub mask: Option<bool>,
    pub fields: Vec<Field>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigFile {
    pub component: Option<Component>,
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
}

#[derive(Default)]
pub struct Generator {
    /// 用于存储配置信息，其内含有requests, responses, components三个目录
    config_dir: PathBuf,
    /// 用于存储生成的.proto文件，其内含有requests, responses, components三个目录
    proto_dir: PathBuf,
    /// 用于存储生成的request pb文件
    request_dir: PathBuf,
    /// 用于存储生成的response pb文件
    response_dir: PathBuf,
    /// 用于存储生成的component pb文件
    component_dir: PathBuf,
}

#[derive(Debug, From)]
pub enum Error {
    Io(std::io::Error),
    Ron(ron::Error, PathBuf),
    DuplicateFieldNumber(String),
    DuplicateCmd,
}

fn read_files(input_dir: PathBuf) -> std::io::Result<Vec<PathBuf>> {
    let mut inputs = Vec::new();
    for f in read_dir(input_dir)? {
        let f = f?;
        if f.file_type()?.is_file() {
            inputs.push(f.path());
        }
    }
    Ok(inputs)
}

impl Generator {
    pub fn config_dir(&mut self, config_dir: impl AsRef<Path>) -> &mut Self {
        self.config_dir = config_dir.as_ref().to_owned();
        self
    }

    pub fn proto_dir(&mut self, proto_dir: impl AsRef<Path>) -> &mut Self {
        self.proto_dir = proto_dir.as_ref().to_owned();
        self
    }

    pub fn request_dir(&mut self, request_dir: impl AsRef<Path>) -> &mut Self {
        self.request_dir = request_dir.as_ref().to_owned();
        self
    }

    pub fn response_dir(&mut self, response_dir: impl AsRef<Path>) -> &mut Self {
        self.response_dir = response_dir.as_ref().to_owned();
        self
    }

    pub fn component_dir(&mut self, component_dir: impl AsRef<Path>) -> &mut Self {
        self.component_dir = component_dir.as_ref().to_owned();
        self
    }

    fn parse_config(config_dir: PathBuf) -> Result<Vec<(PathBuf, ConfigFile)>, Error> {
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

    pub fn run(&mut self) -> Result<(), Error> {
        let empty_path = PathBuf::new();
        if self.request_dir == empty_path {
            self.request_dir = "src/requests".into();
        }
        if self.component_dir == empty_path {
            self.component_dir = "src/components".into();
        }
        if self.response_dir == empty_path {
            self.response_dir = "src/responses".into();
        }
        self.gen_request()?;
        self.gen_response()?;
        self.gen_component()?;
        Ok(())
    }

    fn gen_response(&self) -> Result<(), Error> {
        self.gen_io_config(
            "responses",
            self.response_dir.clone(),
            |mods, names, files, cmds| {
                quote!(
                    #(mod #mods;)*

                    use byteorder::{BigEndian, ByteOrder};
                    use ecs_engine::Output;
                    use protobuf::Message;

                    #(pub use #files::#names;)*

                    #[derive(Debug)]
                    pub enum Response {
                        #(#names(#names),)*
                    }

                    impl Output for Response {
                        #[cfg(feature="debug")]
                        fn decode(mut buffer:&[u8]) ->Option<Self> {
                            let cmd = BigEndian::read_u32(buffer);
                            buffer = &buffer[4..];
                            match cmd {
                            #(
                                    #cmds => {
                                        let mut data = #names::new();
                                        data.merge_from_bytes(buffer).unwrap();
                                        Some(Response::#names(data))
                                    },
                            )*
                                _ => {
                                    log::error!("invalid cmd:{}", cmd);
                                    None
                                },
                            }
                        }

                        fn encode(&self) -> Vec<u8> {
                            let mut data = vec![0u8;8];
                            let cmd = match self {
                                #(
                                    Response::#names(r) => {
                                        r.write_to_vec(&mut data).unwrap();
                                        #cmds
                                    },
                                )*
                            };
                            let length = (data.len() - 4) as u32;
                            let header = data.as_mut_slice();
                            BigEndian::write_u32(header, length);
                            BigEndian::write_u32(&mut header[4..], cmd);
                            data
                        }
                    }
                )
                .to_string()
            },
        )
    }

    fn gen_component(&self) -> Result<(), Error> {
        let mut config_dir = self.config_dir.clone();
        config_dir.push("components");

        let mut proto_dir = self.proto_dir.clone();
        proto_dir.push("components");

        let configs = Self::parse_config(config_dir)?;

        Self::gen_messages(&configs, proto_dir.clone())?;
        Self::gen_protos(proto_dir, self.component_dir.clone())?;

        let mut mods = Vec::new();
        let mut names = Vec::new();
        let mut files = Vec::new();
        let mut storages = Vec::new();
        for (f, cf) in &configs {
            let storage = if let Some(component) = &cf.component {
                component.to_rust_type()
            } else {
                quote!(HashMapStorage<Self>)
            };
            let mod_name = format_ident!("{}", f.file_stem().unwrap().to_str().unwrap());
            mods.push(mod_name.clone());
            for c in &cf.configs {
                files.push(mod_name.clone());
                names.push(format_ident!("{}", c.name));
                storages.push(storage.clone());
            }
        }
        let data = quote!(
            #![allow(unused_imports)]
            #(mod #mods)*;

            use specs::{
                Component, DefaultVecStorage, FlaggedStorage, HashMapStorage, NullStorage,
                VecStorage,
            };
            use std::{
                any::Any,
                ops::{Deref, DerefMut},
            };

            #[derive(Debug, Default)]
            pub struct Type<T:Default> {
                data: T,
            }

            impl<T:Default> Deref for Type<T> {
                type Target = T;

                fn deref(&self) -> &Self::Target {
                    &self.data
                }
            }

            impl<T:Default> DerefMut for Type<T> {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.data
                }
            }

            #(
                impl Component for Type<#files::#names> {
                    type Storage = #storages;
                }
            )*

            #(pub type #names = Type<#files::#names>;)*
        )
        .to_string();
        let mut name = self.component_dir.clone();
        name.push("mod.rs");
        let mut file = File::create(name.clone())?;
        writeln!(
            file,
            "// This file is generated by ecs_engine. Do not edit."
        )?;
        writeln!(file, "// @generated")?;
        file.write_all(data.as_bytes())?;
        drop(file);

        Self::format_file(name)?;
        Ok(())
    }

    fn gen_protos(input_dir: PathBuf, output_dir: PathBuf) -> std::io::Result<()> {
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

    fn string_to_u32(name: &[u8]) -> u32 {
        let digest = md5::compute(name).0;
        BigEndian::read_u32(&digest[..4])
    }

    fn gen_messages(
        configs: &Vec<(PathBuf, ConfigFile)>,
        output_dir: PathBuf,
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
            Self::gen_message(&mut file, &v)?;
        }
        Ok(())
    }

    fn gen_io_config<F>(&self, config_type: &str, dir: PathBuf, codegen: F) -> Result<(), Error>
    where
        F: Fn(Vec<Ident>, Vec<Ident>, Vec<Ident>, Vec<u32>) -> String,
    {
        let mut config_dir = self.config_dir.clone();
        config_dir.push(config_type);

        let mut proto_dir = self.proto_dir.clone();
        proto_dir.push(config_type);

        let configs = Self::parse_config(config_dir)?;

        Self::gen_messages(&configs, proto_dir.clone())?;
        Self::gen_protos(proto_dir, dir.clone())?;

        let mut cmds = Vec::new();
        let mut mods = Vec::new();
        let mut names = Vec::new();
        let mut files = Vec::new();
        for (f, cf) in &configs {
            let mod_name = format_ident!("{}", f.file_stem().unwrap().to_str().unwrap());
            mods.push(mod_name.clone());
            for c in &cf.configs {
                cmds.push(Self::string_to_u32(c.name.as_bytes()));
                files.push(mod_name.clone());
                names.push(format_ident!("{}", c.name));
            }
        }

        let mut n_cmds = cmds.clone();
        let cmd_count = n_cmds.len();
        n_cmds.sort();
        n_cmds.dedup();
        if cmd_count != n_cmds.len() {
            return Err(Error::DuplicateCmd);
        }
        let data = codegen(mods, names, files, cmds);

        let mut name = dir.clone();
        name.push("mod.rs");
        let mut file = File::create(name.clone())?;
        writeln!(
            file,
            "// This file is generated by ecs_engine. Do not edit."
        )?;
        writeln!(file, "// @generated")?;
        file.write_all(data.as_bytes())?;
        drop(file);

        Self::format_file(name)?;
        Ok(())
    }

    fn gen_request(&self) -> Result<(), Error> {
        self.gen_io_config("requests", self.request_dir.clone(), | mods, names, files, cmds| {
            quote!(
            #(mod #mods;)*

            use byteorder::{BigEndian, ByteOrder};
            use ecs_engine::{Closing, HashComponent, Input, NetToken, RequestIdent, ResponseSender, SelfSender};
            use protobuf::Message;
            use specs::{error::Error, World, WorldExt};

            #(pub type #names = HashComponent<#files::#names>;)*

            #[derive(Debug)]
            pub enum Request {
                #(#names(#names),)*
                None,
            }

            impl Input for Request {
                fn add_component(self, ident: RequestIdent, world: &World, sender: &ResponseSender) ->Result<(), Error> {
                    let entity = match ident {
                        RequestIdent::Token(token) => {
                            let entity = world.entities().create();
                            sender.send_entity(token, entity);
                            world.write_component::<NetToken>().insert(entity, NetToken::new(token.0)).map(|_|())?;
                            world.write_component::<SelfSender>().insert(entity, SelfSender::new(token, sender.clone())).map(|_|())?;
                            entity
                        },
                        RequestIdent::Close(entity) => {
                            world.write_component::<Closing>().insert(entity, Closing).map(|_|())?;
                            return Ok(());
                        }
                        RequestIdent::Entity(entity) => entity,
                    };

                    match self {
                        #(Request::#names(c) => world.write_component::<#names>().insert(entity, c).map(|_|()),)*
                        Request::None => Ok(()),
                    }
                }

                fn setup(world:&mut World) {
                    #(world.register::<#names>();)*
                }

                fn decode(mut buffer:&[u8]) ->Option<Self> {
                    if buffer.len() == 0 {
                        return Some(Request::None);
                    }

                    let cmd = BigEndian::read_u32(buffer);
                    buffer = &buffer[4..];
                    match cmd {
                    #(
                            #cmds => {
                                let mut data = #files::#names::new();
                                data.merge_from_bytes(buffer).unwrap();
                                Some(Request::#names(#names::new(data)))
                            },
                    )*
                        _ => {
                            log::error!("invalid cmd:{}", cmd);
                            None
                        },
                    }
                }

                #[cfg(feature="debug")]
                fn encode(&self) -> Vec<u8> {
                    let mut data = vec![0u8;8];
                    let cmd = match self {
                        #(
                            Request::#names(r) => {
                                r.write_to_vec(&mut data).unwrap();
                                #cmds
                            },
                        )*
                        Request::None => 0,
                    };
                    let length = (data.len() - 4) as u32;
                    let header = data.as_mut_slice();
                    BigEndian::write_u32(header, length);
                    BigEndian::write_u32(&mut header[4..], cmd);
                    data
                }
            }
        ).to_string()
        })
    }

    fn format_file(file: PathBuf) -> std::io::Result<()> {
        Command::new("rustfmt").arg(file).output()?;
        Ok(())
    }

    /// 根据Config类型生成一个Protobuf配置文件
    fn gen_message(file: &mut File, cf: &ConfigFile) -> std::io::Result<()> {
        writeln!(file, r#"syntax = "proto3";"#)?;
        for v in &cf.configs {
            writeln!(file, "message {} {{", v.name)?;
            for field in &v.fields {
                writeln!(
                    file,
                    "\t{}{} {} = {};",
                    if field.repeated.is_some() && field.repeated.unwrap() {
                        "repeated "
                    } else {
                        ""
                    },
                    field.r#type.to_pb_type(),
                    field.name,
                    field.index,
                )?;
            }
            if let Some(mask) = v.mask {
                if mask {
                    writeln!(file, "  uint64 mask = {};", v.max_number() + 1)?;
                }
            }
            writeln!(file, "}}")?;
        }
        Ok(())
    }
}
