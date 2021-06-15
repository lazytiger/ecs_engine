use derive_more::From;
use protobuf_codegen_pure::{Codegen, Customize};
use quote::{format_ident, quote};
use serde_derive::Deserialize;
use std::{
    collections::HashMap,
    fs::{read_dir, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Deserialize, Debug)]
pub enum StorageType {
    Vec,
    DefaultVec,
    DenseVec,
    HashMap,
    Null,
}

#[derive(Deserialize, Debug)]
pub enum ConfigType {
    Request,
    Response,
    Component,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
pub struct Component {
    pub flagged: bool,
    pub mask: bool,
    pub r#type: StorageType,
}

#[derive(Deserialize, Debug)]
pub struct Field {
    //pub name: String,
    pub r#type: DataType,
    pub field: u32,
    pub repeated: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub name: String,
    pub r#type: ConfigType,
    pub component: Option<Component>,
    pub fields: HashMap<String, Field>,
}

#[derive(Deserialize, Debug)]
pub struct ConfigFile {
    pub configs: Vec<Config>,
}

impl Config {
    fn max_number(&self) -> u32 {
        if let Some((_, c)) = self
            .fields
            .iter()
            .max_by(|(_, a), (_, b)| a.field.cmp(&b.field))
        {
            c.field
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
    De(toml::de::Error),
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
        let files = read_files(config_dir).map_err(Error::from)?;
        let mut configs = Vec::new();
        for input in files {
            let mut file = File::open(&input).map_err(Error::from)?;
            let mut data = String::new();
            file.read_to_string(&mut data).map_err(Error::from)?;
            let config: ConfigFile = toml::from_str(data.as_str()).map_err(Error::from)?;
            configs.push((input.clone(), config));
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
        Ok(())
    }

    fn gen_component(&self) -> Result<(), Error> {
        Ok(())
    }

    fn gen_protos(input_dir: PathBuf, output_dir: PathBuf) -> std::io::Result<()> {
        let files = read_files(input_dir.clone())?;
        let mut customize = Customize::default();
        //customize.gen_mod_rs = Some(true);
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
        let mut data = [0u8; 4];
        data.copy_from_slice(&digest[..4]);
        unsafe { std::mem::transmute::<[u8; 4], u32>(data) }
    }

    fn gen_messages(
        configs: &Vec<(PathBuf, ConfigFile)>,
        output_dir: PathBuf,
    ) -> Result<(), Error> {
        for (k, v) in configs {
            let mut name = k.file_stem().unwrap().to_owned();
            name.push(".proto");
            let mut path = output_dir.clone();
            path.push(name);
            let mut file = File::create(path).map_err(Error::from)?;
            Self::gen_message(&mut file, &v).map_err(Error::from)?;
        }
        Ok(())
    }

    fn gen_request(&self) -> Result<(), Error> {
        let mut config_dir = self.config_dir.clone();
        config_dir.push("requests");

        let mut proto_dir = self.proto_dir.clone();
        proto_dir.push("requests");

        let configs = Self::parse_config(config_dir)?;

        Self::gen_messages(&configs, proto_dir.clone());
        Self::gen_protos(proto_dir, self.request_dir.clone());

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

        let data = quote!(
            #(mod #mods;)*
            use ecs_engine::network::Input;
            use ecs_engine::ReadOnly;
            use specs::World;
            use specs::Entity;
            use specs::WorldExt;
            use specs::HashMapStorage;
            use specs::Component;
            use specs::error::Error;
            use std::ops::Deref;
            use protobuf::Message;

            pub struct ComponentWrapper<T> {
                data:T
            }

            impl<T> Deref for ComponentWrapper<T> {
                type Target = T;

                fn deref(&self) -> &Self::Target {
                    &self.data
                }
            }

            impl<T: 'static + Send + Sync> Component for ComponentWrapper<T> {
                type Storage = HashMapStorage<Self>;
            }

            #(pub type #names = ComponentWrapper<#files::#names>;)*

            pub enum Request {
                #(#names(#names),)*
            }

            impl Input for Request {
                fn add_component(self, entity:Option<Entity>, world:&World) ->Result<(), Error> {
                    match self {
                        #(Request::#names(c) => world.write_component::<#names>().insert(entity.unwrap(), c).map(|_|()),)*
                    }
                }

                fn setup(world:&mut World) {
                    #(world.register::<#names>();)*
                }

                fn decode(cmd:u32, buffer:&[u8]) ->Self {
                    match cmd {
                    #(
                            #cmds => {
                                let mut data = #files::#names::new();
                                data.merge_from_bytes(buffer).unwrap();
                                Request::#names(ComponentWrapper{data})
                            },
                    )*
                        _ => panic!("unexpected cmd {}", cmd),
                    }
                }
            }
        )
        .to_string();

        let mut name = self.request_dir.clone();
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

    fn format_file(file: PathBuf) -> std::io::Result<()> {
        Command::new("rustfmt").arg(file).output()?;
        Ok(())
    }

    /// 根据Config类型生成一个Protobuf配置文件
    fn gen_message(file: &mut File, cf: &ConfigFile) -> std::io::Result<()> {
        writeln!(file, r#"syntax = "proto3";"#)?;
        for v in &cf.configs {
            writeln!(file, "message {} {{", v.name)?;
            for (name, c) in &v.fields {
                writeln!(
                    file,
                    "\t{}{} {} = {};",
                    if c.repeated.is_some() && c.repeated.unwrap() {
                        "repeated "
                    } else {
                        ""
                    },
                    c.r#type.to_rust_type(),
                    name,
                    c.field
                )?;
            }
            if let ConfigType::Component = v.r#type {
                let mask = if let Some(component) = &v.component {
                    component.mask
                } else {
                    false
                };
                if mask {
                    writeln!(file, "u64 mask = {};", v.max_number() + 1)?;
                }
            }
            writeln!(file, "}}")?;
        }
        Ok(())
    }
}
