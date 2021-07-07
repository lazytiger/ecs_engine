use std::{
    fs::{read_dir, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use byteorder::{BigEndian, ByteOrder};
use derive_more::From;
use proc_macro2::{Ident, TokenStream};
use protobuf_codegen_pure::{Codegen, Customize};
use quote::{format_ident, quote};
use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum SyncDirection {
    Client,
    Database,
    Team,
    Around,
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
    List(Box<DataType>),
    Map(Box<DataType>, Box<DataType>),
    Custom(String),
}

impl DataType {
    fn to_pb_type(&self) -> String {
        match self {
            DataType::String => "string".into(),
            DataType::U32 => "uint32".into(),
            DataType::U64 => "uint64".into(),
            DataType::S32 => "sint32".into(),
            DataType::S64 => "sint64".into(),
            DataType::F32 => "float".into(),
            DataType::F64 => "double".into(),
            DataType::Bool => "bool".into(),
            DataType::Bytes => "bytes".into(),
            DataType::Custom(name) => name.clone(),
            DataType::List(name) => format!("repeated {}", name.to_pb_type()),
            DataType::Map(key, value) => {
                format!("map<{}, {}>", key.to_pb_type(), value.to_pb_type())
            }
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
    pub component: Option<Component>,
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
                    match dir {
                        SyncDirection::Client => mask |= 1,
                        SyncDirection::Database => mask |= 1 << 1,
                        SyncDirection::Team => mask |= 1 << 2,
                        SyncDirection::Around => mask |= 1 << 3,
                    }
                }
            } else {
                return 0x0f;
            }
        }
        mask
    }
}

#[derive(Default)]
pub struct Generator {
    /// 用于存储配置信息，其内含有request, response, dataset三个目录
    config_dir: PathBuf,
    /// 用于存储生成的.proto文件，其内含有request, response, dataset三个目录
    proto_dir: PathBuf,
    /// 用于存储生成的request pb文件
    request_dir: PathBuf,
    /// 用于存储生成的response pb文件
    response_dir: PathBuf,
    /// 用于存储生成的dataset pb文件
    dataset_dir: PathBuf,
}

#[derive(Debug, From)]
pub enum Error {
    Io(std::io::Error),
    Ron(ron::Error, PathBuf),
    DuplicateFieldNumber(String),
    DuplicateCmd,
    ComponentListUsed(PathBuf, String),
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

    pub fn dataset_dir(&mut self, dataset_dir: impl AsRef<Path>) -> &mut Self {
        self.dataset_dir = dataset_dir.as_ref().to_owned();
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
            self.request_dir = "src/request".into();
        }
        if self.dataset_dir == empty_path {
            self.dataset_dir = "src/dataset".into();
        }
        if self.response_dir == empty_path {
            self.response_dir = "src/response".into();
        }
        self.gen_request()?;
        self.gen_response()?;
        self.gen_dataset()?;
        Ok(())
    }

    fn gen_response(&self) -> Result<(), Error> {
        self.gen_io_config(
            "response",
            self.response_dir.clone(),
            |mods, names, files, inners, cmds| {
                quote!(
                    #(mod #mods;)*

                    use byteorder::{BigEndian, ByteOrder};
                    use ecs_engine::Output;
                    use protobuf::Message;
                    use derive_more::From;
                    pub type SelfSender = ecs_engine::SelfSender<Response>;

                    #(pub use #files::#names;)*
                    #(pub use #inners;)*

                    #[derive(Debug, From, Clone)]
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

    fn gen_dataset(&self) -> Result<(), Error> {
        let mut config_dir = self.config_dir.clone();
        config_dir.push("dataset");

        let mut proto_dir = self.proto_dir.clone();
        proto_dir.push("dataset");

        let configs = Self::parse_config(config_dir)?;
        for (path, cf) in &configs {
            for config in &cf.configs {
                for f in &config.fields {
                    if let DataType::List(_) = f.r#type {
                        return Err(Error::ComponentListUsed(path.clone(), config.name.clone()));
                    }
                    if let DataType::Map(_, v) = &f.r#type {
                        if let DataType::Custom(_) = v.as_ref() {
                            continue;
                        } else {
                            return Err(Error::ComponentListUsed(
                                path.clone(),
                                config.name.clone(),
                            ));
                        }
                    }
                }
            }
        }

        Self::gen_messages(&configs, proto_dir.clone(), true)?;
        Self::gen_protos(proto_dir, self.dataset_dir.clone())?;

        let mut mods = Vec::new();
        let mut names = Vec::new();
        let mut files = Vec::new();
        let mut storages = Vec::new();
        let mut inners = Vec::new();
        let mut cs_codes = Vec::new();
        let mut indexes = Vec::new();
        let mut ns = Vec::new();
        let mut cmds = Vec::new();
        let mut vnames = Vec::new();
        let all_dirs = vec![
            SyncDirection::Team,
            SyncDirection::Database,
            SyncDirection::Around,
            SyncDirection::Client,
        ];
        for (f, cf) in &configs {
            let mod_name = format_ident!("{}", f.file_stem().unwrap().to_str().unwrap());
            mods.push(mod_name.clone());
            for c in &cf.configs {
                let vname = c.name.clone();
                vnames.push(vname.clone());
                let name = format_ident!("{}", c.name);
                if let Some(component) = &c.component {
                    files.push(mod_name.clone());
                    names.push(name.clone());
                    storages.push(component.to_rust_type());
                    ns.push(c.get_dir_mask());
                    cmds.push(Self::string_to_u32(vname.as_bytes()));
                } else {
                    inners.push(quote!(#mod_name::#name));
                }
                indexes.push(indexes.len());
                let mut client_mask = 0u64;
                let mut around_mask = 0u64;
                let mut database_mask = 0u64;
                let mut team_mask = 0u64;
                let mut single_numbers = Vec::new();
                let mut single_names = Vec::new();
                let mut map_numbers = Vec::new();
                let mut map_names = Vec::new();

                for f in &c.fields {
                    let dirs = f.dirs.as_ref().unwrap_or(&all_dirs);
                    let mask = 1 << (f.index as u64);
                    for dir in dirs {
                        match dir {
                            SyncDirection::Client => client_mask |= mask,
                            SyncDirection::Database => database_mask |= mask,
                            SyncDirection::Team => team_mask |= mask,
                            SyncDirection::Around => around_mask |= mask,
                        }
                    }
                    let index = f.index as usize;
                    match f.r#type {
                        DataType::Custom(_) => {
                            single_numbers.push(index);
                            single_names.push(format_ident!("get_{}", f.name));
                        }
                        DataType::Map(..) => {
                            map_numbers.push(index);
                            map_names.push(format_ident!("get_{}", f.name));
                        }
                        _ => {}
                    }
                }
                let cs_code = quote! {
                    impl DirectionMask for #mod_name::#name {

                        #[allow(unused_variables)]
                        fn mask_by_direction(&self, dir:SyncDirection, ms: &mut MaskSet) {
                            let mask = match dir {
                                SyncDirection::Client => #client_mask,
                                SyncDirection::Around => #around_mask,
                                SyncDirection::Database => #database_mask,
                                SyncDirection::Team => #team_mask,
                            };
                            ms.mask &= mask;
                            ms.set.iter_mut().for_each(|(k, set)| {
                                match *k {
                                    #(
                                        #single_numbers => {
                                            if let Some(ms) = set.get_mut(&(0.into())) {
                                                self.#single_names().mask_by_direction(dir, ms);
                                            }
                                        }
                                    )*
                                    #(
                                        #map_numbers => {
                                            self.#map_names().iter().for_each(|(k, f)|{
                                                if let Some(ms)  = set.get_mut(&(k.clone().into()))  {
                                                    f.mask_by_direction(dir, ms);
                                                }
                                            });
                                        }
                                    )*
                                    _ => panic!("unknown field in {}", #vname),
                                }
                            })
                        }
                    }
                };
                cs_codes.push(cs_code);
            }
        }
        let data = quote!(
            #![allow(unused_imports)]
            #(mod #mods)*;

            use specs::{
                Component, DefaultVecStorage, FlaggedStorage, HashMapStorage, NullStorage,
                VecStorage, DispatcherBuilder, Tracked,
            };
            use std::{
                any::Any,
                ops::{Deref, DerefMut},
            };
            use protobuf::{Message, MaskSet, Mask};
            use ecs_engine::{ChangeSet, SyncDirection, DataSet, CommitChangeSystem, SceneData};
            use byteorder::{BigEndian, ByteOrder};
            #(pub use #inners;)*

            #[derive(Debug, Default, Clone)]
            pub struct Type<T:Default+Clone, const N: usize, const C: u32> {
                data: T,
                database_mask: Option<MaskSet>,
                client_mask: Option<MaskSet>,
                around_mask: Option<MaskSet>,
                team_mask: Option<MaskSet>,
            }

            impl<T:Message + Default + Clone, const N:usize, const C: u32> Type<T, N, C> {
                pub fn new() ->Self {
                    let client_mask = if N & 0x1 != 0 {
                        Some(MaskSet::default())
                    } else {
                        None
                    };
                    let database_mask = if N & 0x02 != 0 {
                        Some(MaskSet::default())
                    } else {
                        None
                    };
                    let team_mask = if N & 0x04 != 0 {
                        Some(MaskSet::default())
                    } else {
                        None
                    };
                    let around_mask = if N & 0x08 != 0 {
                        Some(MaskSet::default())
                    } else {
                        None
                    };
                    Self {
                        data:T::new(),
                        client_mask,
                        database_mask,
                        team_mask,
                        around_mask,
                    }
                }

            }

            impl<T:Message + Default + Mask + DirectionMask + Clone, const N:usize, const C:u32> DataSet for Type<T, N, C> {
                fn commit(&mut self) {
                    let mut ms = None;
                    if self.client_mask.is_some() {
                        let ms = ms.get_or_insert_with(||self.data.mask_set());
                        *self.client_mask.as_mut().unwrap() |= ms;
                    }
                    if self.database_mask.is_some() {
                        let ms = ms.get_or_insert_with(||self.data.mask_set());
                        *self.database_mask.as_mut().unwrap() |= ms;
                    }
                    if self.team_mask.is_some() {
                        let ms = ms.get_or_insert_with(||self.data.mask_set());
                        *self.team_mask.as_mut().unwrap() |= ms;
                    }
                    if self.around_mask.is_some() {
                        let ms = ms.get_or_insert_with(||self.data.mask_set());
                        *self.around_mask.as_mut().unwrap() |= ms;
                    }
                    self.data.clear_mask();
                }

                fn encode(&mut self, dir:SyncDirection) ->Option<Vec<u8>> {
                    let mask = match dir {
                        SyncDirection::Client => {
                            if let Some(mask) = &mut self.client_mask {
                                self.data.mask_by_direction(dir, mask);
                                mask
                            } else {
                                return None;
                            }
                        }
                        SyncDirection::Database =>  {
                            if let Some(mask) = &mut self.database_mask {
                                self.data.mask_by_direction(dir, mask);
                                mask
                            } else {
                                return None;
                            }
                        }
                        SyncDirection::Team =>  {
                            if let Some(mask) = &mut self.team_mask {
                                self.data.mask_by_direction(dir, mask);
                                mask
                            } else {
                                return None;
                            }
                        }
                        SyncDirection::Around => {
                            if let Some(mask) = &mut self.around_mask {
                                self.data.mask_by_direction(dir, mask);
                                mask
                            } else {
                                return None;
                            }
                        }
                    };
                    let mut data = vec![0u8; 8];
                    self.data.set_mask(mask);
                    if let Err(err) = self.data.write_to_vec(&mut data) {
                        log::error!("encode data failed:{}", err);
                        return None;
                    } else {
                        let length = (data.len() - 4) as u32;
                        let header = data.as_mut_slice();
                        BigEndian::write_u32(header, length);
                        BigEndian::write_u32(&mut header[4..], C);
                    }
                    self.data.clear_mask();
                    mask.clear();
                    Some(data)
                }

                fn is_data_dirty(&self) -> bool {
                    self.data.is_dirty()
                }

                fn is_direction_enabled(&self, dir:SyncDirection) -> bool {
                    match dir {
                        SyncDirection::Client => self.client_mask.is_some(),
                        SyncDirection::Database => self.database_mask.is_some(),
                        SyncDirection::Team => self.team_mask.is_some(),
                        SyncDirection::Around => self.around_mask.is_some(),
                    }
                }
            }

            impl<T:Default + Clone, const N:usize, const C:u32> Deref for Type<T, N, C> {
                type Target = T;

                fn deref(&self) -> &Self::Target {
                    &self.data
                }
            }

            impl<T:Default + Clone, const N:usize, const C:u32> DerefMut for Type<T, N, C> {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.data
                }
            }

            #(
                impl Component for Type<#files::#names, #ns, #cmds> {
                    type Storage = #storages;
                }

                impl ChangeSet for Type<#files::#names, #ns, #cmds> {
                    fn index() -> usize {
                        #indexes
                    }
                }
                pub type #names = Type<#files::#names, #ns, #cmds>;
            )*

            pub trait DirectionMask {
                fn mask_by_direction(&self, direction: SyncDirection, ms: &mut MaskSet);
            }

            #(#cs_codes)*

            pub fn setup<P, S>(builder:&mut DispatcherBuilder)
            where
                P: Component + ecs_engine::Position + Send + Sync + 'static,
                P::Storage: Tracked,
                S: Component + SceneData + Send + Sync + 'static,
                S::Storage: Tracked,
            {
                #(
                    builder.add(CommitChangeSystem::<#names, P, S>::default(), #vnames, &[]);
                )*
            }
        )
        .to_string();
        let mut name = self.dataset_dir.clone();
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
            Self::gen_message(&mut file, &v, mask)?;
        }
        Ok(())
    }

    fn gen_io_config<F>(&self, config_type: &str, dir: PathBuf, codegen: F) -> Result<(), Error>
    where
        F: Fn(Vec<Ident>, Vec<Ident>, Vec<Ident>, Vec<TokenStream>, Vec<u32>) -> String,
    {
        let mut config_dir = self.config_dir.clone();
        config_dir.push(config_type);

        let mut proto_dir = self.proto_dir.clone();
        proto_dir.push(config_type);

        let configs = Self::parse_config(config_dir)?;

        Self::gen_messages(&configs, proto_dir.clone(), false)?;
        Self::gen_protos(proto_dir, dir.clone())?;

        let mut cmds = Vec::new();
        let mut mods = Vec::new();
        let mut names = Vec::new();
        let mut files = Vec::new();
        let mut inners = Vec::new();
        for (f, cf) in &configs {
            let mod_name = format_ident!("{}", f.file_stem().unwrap().to_str().unwrap());
            mods.push(mod_name.clone());
            for c in &cf.configs {
                let name = format_ident!("{}", c.name);
                if let Some(true) = c.hide {
                    inners.push(quote!(#mod_name::#name));
                } else {
                    cmds.push(Self::string_to_u32(c.name.as_bytes()));
                    files.push(mod_name.clone());
                    names.push(name);
                }
            }
        }

        let mut n_cmds = cmds.clone();
        let cmd_count = n_cmds.len();
        n_cmds.sort();
        n_cmds.dedup();
        if cmd_count != n_cmds.len() {
            return Err(Error::DuplicateCmd);
        }
        let data = codegen(mods, names, files, inners, cmds);

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
        self.gen_io_config("request", self.request_dir.clone(), | mods, names, files,inners, cmds| {
            quote!(
            #(mod #mods;)*

            use byteorder::{BigEndian, ByteOrder};
            use derive_more::From;
            use ecs_engine::{Closing, HashComponent, Input, NetToken, RequestIdent, ResponseSender, SelfSender};
            use protobuf::Message;
            use specs::{error::Error, World, WorldExt};
            use crate::response::Response;

            #(pub type #names = HashComponent<#files::#names>;)*
            #(pub use #inners;)*

            #[derive(Debug, From)]
            pub enum Request {
                #(#names(#names),)*
                None,
            }

            impl Input for Request {
                type Output = Response;
                fn add_component(self, ident: RequestIdent, world: &World, sender: ResponseSender<Self::Output>) ->Result<(), Error> {
                    let entity = match ident {
                        RequestIdent::Token(token) => {
                            let entity = world.entities().create();
                            sender.send_entity(token, entity);
                            world.write_component::<NetToken>().insert(entity, NetToken::new(token.0)).map(|_|())?;
                            world.write_component::<SelfSender<Self::Output>>().insert(entity, SelfSender::new(token, sender)).map(|_|())?;
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
    fn gen_message(file: &mut File, cf: &ConfigFile, mask: bool) -> std::io::Result<()> {
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
}
