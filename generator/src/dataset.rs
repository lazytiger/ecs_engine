use crate::{
    format_file, gen_messages, gen_protos, parse_config, string_to_u32, ConfigFile, DataType,
    Error, SyncDirection, Trait,
};
use bytes::BytesMut;
use convert_case::{Case, Casing};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use std::{fmt::Write as _, fs::File, io::Write, path::PathBuf};

fn validate(configs: &Vec<(PathBuf, ConfigFile)>) -> Result<(), Error> {
    for (path, cf) in configs {
        for config in &cf.configs {
            for f in &config.fields {
                if let DataType::List { .. } = f.r#type {
                    return Err(Error::ComponentListUsed(path.clone(), config.name.clone()));
                }
                if let DataType::Map { value, .. } = &f.r#type {
                    if let DataType::Custom { .. } = value.as_ref() {
                        continue;
                    } else {
                        return Err(Error::ComponentListUsed(path.clone(), config.name.clone()));
                    }
                }
            }
            if let Some(indexes) = &config.indexes {
                let mut names: Vec<_> = indexes.iter().map(|index| &index.name).collect();
                names.sort();
                names.dedup();
                if names.len() != indexes.len() {
                    return Err(Error::DuplicateIndexName);
                }

                for index in indexes {
                    let mut names = index.columns.clone();
                    names.sort();
                    names.dedup();
                    if names.len() != index.columns.len() {
                        return Err(Error::DuplicateIndexColumn);
                    }
                    for column in &index.columns {
                        if !config.fields.iter().any(|field| &field.name == column) {
                            return Err(Error::InvalidIndexColumnName);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn gen_position_code(name: &Ident, x: &Option<String>, y: &Option<String>) -> TokenStream {
    let x = format_ident!("{}", x.clone().unwrap_or("get_x".into()));
    let y = format_ident!("{}", y.clone().unwrap_or("get_y".into()));

    quote!(
        impl ecs_engine::Position for #name {
            fn x(&self) -> f32 {
                self.data.#x()
            }
            fn y(&self) -> f32 {
                self.data.#y()
            }
        }
    )
}

fn gen_scene_data_code(
    name: &Ident,
    id: &Option<String>,
    min_x: &Option<String>,
    min_y: &Option<String>,
    row: &Option<String>,
    column: &Option<String>,
    grid_size: &Option<String>,
) -> TokenStream {
    let id = format_ident!("{}", id.clone().unwrap_or("get_id".into()));
    let min_x = format_ident!("{}", min_x.clone().unwrap_or("get_min_x".into()));
    let min_y = format_ident!("{}", min_y.clone().unwrap_or("get_min_y".into()));
    let row = format_ident!("{}", row.clone().unwrap_or("get_row".into()));
    let column = format_ident!("{}", column.clone().unwrap_or("get_column".into()));
    let grid_size = format_ident!("{}", grid_size.clone().unwrap_or("get_grid_size".into()));
    quote!(
        impl ecs_engine::SceneData for #name {
            fn id(&self) -> u32 {
                self.data.#id()
            }

            fn get_min_x(&self) -> f32 {
                self.data.#min_x()
            }

            fn get_min_y(&self) -> f32 {
                self.data.#min_y()
            }

            fn get_column(&self) -> i32 {
                self.data.#column()
            }

            fn get_row(&self) -> i32 {
                self.data.#row()
            }

            fn grid_size(&self) -> f32 {
                self.data.#grid_size()
            }
        }
    )
}

fn gen_backend_code(
    mod_name: &Ident,
    name: &Ident,
    table_name: &String,
    select: &String,
    columns: &Vec<TokenStream>,
) -> TokenStream {
    quote! {
        impl MysqlBackend for #mod_name::#name {
            fn table_def() -> Table {
                let mut table = Table::default();
                table.set_engine("InnoDb");
                table.set_charset("utf8mb4");
                table.set_name(#table_name);
                #(
                    #columns
                    table.columns.push(column);
                )*
                table
            }

            fn load(&mut self, conn:&mut mysql::PooledConn) -> Result<(), mysql::Error> {
                conn.query_first(#select, Params::Empty)?
            }

            fn save(&self, conn:&mut mysql::PooledConn) -> Result<(), mysql::Error> {
                Ok(())
            }
        }
    }
}

fn gen_dm_code(
    vname: &String,
    mod_name: &Ident,
    name: &Ident,
    client_mask: u64,
    around_mask: u64,
    database_mask: u64,
    team_mask: u64,
    single_numbers: &Vec<usize>,
    single_names: &Vec<Ident>,
    map_numbers: &Vec<usize>,
    map_names: &Vec<Ident>,
) -> TokenStream {
    quote! {
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
    }
}

fn gen_dataset_type() -> TokenStream {
    quote!(
        #[derive(Debug, Default, Clone)]
        pub struct Type<T: Default + Clone, const N: usize, const C: u32> {
            data: T,
            database_mask: Option<MaskSet>,
            client_mask: Option<MaskSet>,
            around_mask: Option<MaskSet>,
            team_mask: Option<MaskSet>,
        }

        impl<T: Message + Default + Clone, const N: usize, const C: u32> Type<T, N, C> {
            pub fn new() -> Self {
                let around_mask: usize = SyncDirection::Around.into();
                let client_mask: usize = SyncDirection::Client.into();
                let database_mask: usize = SyncDirection::Database.into();
                let team_mask: usize = SyncDirection::Team.into();

                let around_mask = if N & around_mask != 0 {
                    Some(MaskSet::default())
                } else {
                    None
                };

                let client_mask = if N & client_mask != 0 {
                    Some(MaskSet::default())
                } else {
                    None
                };
                let database_mask = if N & database_mask != 0 {
                    Some(MaskSet::default())
                } else {
                    None
                };
                let team_mask = if N & team_mask != 0 {
                    Some(MaskSet::default())
                } else {
                    None
                };
                Self {
                    data: T::new(),
                    client_mask,
                    database_mask,
                    team_mask,
                    around_mask,
                }
            }
        }

        impl<T: Message + Default + Mask + DirectionMask + Clone, const N: usize, const C: u32>
            DataSet for Type<T, N, C>
        {
            fn commit(&mut self) {
                let mut ms = None;
                if self.client_mask.is_some() {
                    let ms = ms.get_or_insert_with(|| self.data.mask_set());
                    *self.client_mask.as_mut().unwrap() |= ms;
                }
                if self.database_mask.is_some() {
                    let ms = ms.get_or_insert_with(|| self.data.mask_set());
                    *self.database_mask.as_mut().unwrap() |= ms;
                }
                if self.team_mask.is_some() {
                    let ms = ms.get_or_insert_with(|| self.data.mask_set());
                    *self.team_mask.as_mut().unwrap() |= ms;
                }
                if self.around_mask.is_some() {
                    let ms = ms.get_or_insert_with(|| self.data.mask_set());
                    *self.around_mask.as_mut().unwrap() |= ms;
                }
                self.data.clear_mask();
            }

            fn encode(&mut self, id: u32, dir: SyncDirection) -> Option<Vec<u8>> {
                let mask = match dir {
                    SyncDirection::Client => {
                        if let Some(mask) = &mut self.client_mask {
                            self.data.mask_by_direction(dir, mask);
                            mask
                        } else {
                            return None;
                        }
                    }
                    SyncDirection::Database => {
                        if let Some(mask) = &mut self.database_mask {
                            self.data.mask_by_direction(dir, mask);
                            mask
                        } else {
                            return None;
                        }
                    }
                    SyncDirection::Team => {
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
                let mut data = vec![0u8; 12];
                self.data.set_mask(mask);
                if let Err(err) = self.data.write_to_vec(&mut data) {
                    log::error!("encode data failed:{}", err);
                    return None;
                } else {
                    let length = (data.len() - 4) as u32;
                    let header = data.as_mut_slice();
                    BigEndian::write_u32(header, length);
                    BigEndian::write_u32(&mut header[4..], id);
                    BigEndian::write_u32(&mut header[8..], C);
                }
                self.data.clear_mask();
                mask.clear();
                Some(data)
            }

            fn is_data_dirty(&self) -> bool {
                self.data.is_dirty()
            }

            fn is_direction_enabled(dir: SyncDirection) -> bool {
                let mask: usize = dir.into();
                mask & N != 0
            }
        }

        impl<T: Default + Clone, const N: usize, const C: u32> Deref for Type<T, N, C> {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                &self.data
            }
        }

        impl<T: Default + Clone, const N: usize, const C: u32> DerefMut for Type<T, N, C> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.data
            }
        }
    )
}

pub fn gen_dataset(
    dataset_dir: PathBuf,
    mut config_dir: PathBuf,
    mut proto_dir: PathBuf,
) -> Result<(), Error> {
    config_dir.push("dataset");
    proto_dir.push("dataset");

    let configs = parse_config(config_dir)?;
    validate(&configs)?;

    gen_messages(&configs, proto_dir.clone(), true)?;
    gen_protos(proto_dir, dataset_dir.clone())?;

    let mut mods = Vec::new();
    let mut names = Vec::new();
    let mut files = Vec::new();
    let mut storages = Vec::new();
    let mut inners = Vec::new();
    let mut dm_codes = Vec::new();
    let mut backend_codes = Vec::new();
    let mut ns = Vec::new();
    let mut cmds = Vec::new();
    let mut vnames = Vec::new();
    let all_dirs = vec![
        SyncDirection::Team,
        SyncDirection::Database,
        SyncDirection::Around,
        SyncDirection::Client,
    ];
    let mut position_code = quote!();
    let mut scene_data_code = quote!();
    for (f, cf) in &configs {
        let mod_name = format_ident!("{}", f.file_stem().unwrap().to_str().unwrap());
        mods.push(mod_name.clone());
        for c in &cf.configs {
            let vname = c.name.clone();
            vnames.push(vname.clone());
            let name = format_ident!("{}", c.name);
            if let Some(traits) = &c.traits {
                for t in traits {
                    match t {
                        Trait::Component { .. } => {
                            files.push(mod_name.clone());
                            names.push(name.clone());
                            storages.push(t.to_rust_type());
                            ns.push(c.get_dir_mask());
                            cmds.push(string_to_u32(vname.as_bytes()));
                        }
                        Trait::Position { x, y } => {
                            if !position_code.is_empty() {
                                return Err(Error::DuplicatePosition);
                            }
                            position_code = gen_position_code(&name, x, y);
                        }
                        Trait::SceneData {
                            id,
                            min_x,
                            min_y,
                            row,
                            column,
                            grid_size,
                        } => {
                            if !scene_data_code.is_empty() {
                                return Err(Error::DuplicateSceneData);
                            }
                            scene_data_code =
                                gen_scene_data_code(&name, id, min_x, min_y, row, column, grid_size)
                        }
                        Trait::DropEntity { .. } => {
                            return Err(Error::InvalidDropEntity);
                        }
                    }
                    if let Trait::Component { .. } = t {}
                }
            } else {
                inners.push(quote!(#mod_name::#name));
            }
            let mut client_mask = 0u64;
            let mut around_mask = 0u64;
            let mut database_mask = 0u64;
            let mut team_mask = 0u64;
            let mut single_numbers = Vec::new();
            let mut single_names = Vec::new();
            let mut map_numbers = Vec::new();
            let mut map_names = Vec::new();
            let mut columns = Vec::new();

            let mut select = BytesMut::new();
            let mut condition = BytesMut::new();
            write!(select, "SELECT").unwrap();
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
                    DataType::Custom { .. } => {
                        single_numbers.push(index);
                        single_names.push(format_ident!("get_{}", f.name));
                    }
                    DataType::Map { .. } => {
                        map_numbers.push(index);
                        map_names.push(format_ident!("get_{}", f.name));
                    }
                    _ => {}
                }
                let field = &f.name;
                let field_type = f.r#type.to_db_type();
                if database_mask & mask != 0 {
                    write!(select, " `{}`,", field).unwrap();
                    let column = quote!(
                        let mut column = Column::default();
                        column.field = #field.into();
                        column.field_type = #field_type.into();
                        column.default = None;
                        column.null = BoolValue::No;
                    );
                    columns.push(column);
                }
            }
            select.truncate(select.len() - 1);
            let table_name = vname.to_case(Case::Snake);
            write!(select, " FROM `{}` WHERE {}", table_name, unsafe {
                String::from_utf8_unchecked(condition.to_vec())
            })
            .unwrap();
            let select = unsafe { String::from_utf8_unchecked(select.to_vec()) };

            let backend_code = gen_backend_code(&mod_name, &name, &table_name, &select, &columns);
            backend_codes.push(backend_code);

            let dm_code = gen_dm_code(
                &vname,
                &mod_name,
                &name,
                client_mask,
                around_mask,
                database_mask,
                team_mask,
                &single_numbers,
                &single_names,
                &map_numbers,
                &map_names,
            );
            dm_codes.push(dm_code);
        }
    }
    let dataset_type_code = gen_dataset_type();

    let data = quote!(
            #![allow(unused_imports)]
            #(mod #mods;)*

            use specs::{
                Component, DefaultVecStorage, FlaggedStorage, HashMapStorage, NullStorage,
                VecStorage,  Tracked, WorldExt, World,
            };
            use std::{
                any::Any,
                ops::{Deref, DerefMut},
            };
            use protobuf::{Message, MaskSet, Mask};
            use ecs_engine::{SyncDirection, DataSet, CommitChangeSystem, GameDispatcherBuilder, SceneSyncBackend};
            use byteorder::{BigEndian, ByteOrder};
            use dataproxy::{Table, BoolValue, Column, Index};
            #(pub use #inners;)*

            #dataset_type_code

            #(
                impl Component for Type<#files::#names, #ns, #cmds> {
                    type Storage = #storages;
                }

                pub type #names = Type<#files::#names, #ns, #cmds>;
            )*

            pub trait DirectionMask {
                fn mask_by_direction(&self, direction: SyncDirection, ms: &mut MaskSet);
            }
            #(#dm_codes)*

            pub trait MysqlBackend {
                fn table_def() -> Table;

                fn load(&mut self, conn:&mut mysql::PooledConn) -> Result<(), mysql::Error>;

                fn save(&self, conn:&mut mysql::PooledConn) -> Result<(), mysql::Error>;
            }
            #(#backend_codes)*

            #position_code
            #scene_data_code

            pub fn setup<B>(world:&mut World, builder:&mut GameDispatcherBuilder)
            where
                B: SceneSyncBackend + Send + Sync + 'static,
                <<B as SceneSyncBackend>::Position as Component>::Storage: Tracked + Default,
                <<B as SceneSyncBackend>::SceneData as Component>::Storage: Tracked + Default,
            {
                #(
                    builder.add(CommitChangeSystem::<#names, B>::new(world), #vnames, &[]);
                )*
            }
        )
        .to_string();
    let mut name = dataset_dir.clone();
    name.push("mod.rs");
    let mut file = File::create(name.clone())?;
    writeln!(
        file,
        "// This file is generated by ecs_engine. Do not edit."
    )?;
    writeln!(file, "// @generated")?;
    file.write_all(data.as_bytes())?;
    drop(file);

    format_file(name)?;
    Ok(())
}
