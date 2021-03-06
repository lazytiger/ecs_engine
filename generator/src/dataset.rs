use crate::{
    format_file, gen_messages, gen_protos, parse_config, string_to_u32, ConfigFile, DataType,
    Error, IndexType, SyncDirection, Trait,
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
                    return Err(Error::ComponentListUsed(
                        path.clone(),
                        config.name.clone(),
                        f.name.clone(),
                    ));
                }
                if let DataType::Map { value, .. } = &f.r#type {
                    if config.hide.is_none() {
                        return Err(Error::MapUsedAsRootDatasetType(
                            path.clone(),
                            config.name.clone(),
                            f.name.clone(),
                        ));
                    }
                    if let DataType::Custom { .. } = value.as_ref() {
                        continue;
                    } else {
                        return Err(Error::ComponentListUsed(
                            path.clone(),
                            config.name.clone(),
                            f.name.clone(),
                        ));
                    }
                }
            }
            if let Some(indexes) = &config.indexes {
                for (index_type, index) in indexes {
                    let mut names = index.columns.clone();
                    names.sort();
                    names.dedup();
                    if names.len() != index.columns.len() {
                        return Err(Error::DuplicateIndexColumn(
                            path.clone(),
                            config.name.clone(),
                            index_type.clone(),
                        ));
                    }
                    for column in &index.columns {
                        if !config.is_database_column(column.as_str()) {
                            return Err(Error::InvalidIndexColumnName(
                                path.clone(),
                                config.name.clone(),
                                index_type.clone(),
                            ));
                        }
                        let field = config.get_field(column.as_str()).unwrap();
                        if !match field.r#type {
                            DataType::U32 { .. } => true,
                            DataType::U64 => true,
                            DataType::S32 { .. } => true,
                            DataType::S64 => true,
                            DataType::String { .. } => true,
                            _ => false,
                        } {
                            return Err(Error::InvalidIndexColumnType(
                                path.clone(),
                                config.name.clone(),
                                index_type.clone(),
                            ));
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
    name: &Ident,
    table_name: &String,
    select: &String,
    insert: &String,
    update: &String,
    delete: &String,
    columns: &Vec<TokenStream>,
    indexes: &Vec<TokenStream>,
    fields: &Vec<Ident>,
    field_types: &Vec<TokenStream>,
    customs: &Vec<u32>,
    conds: &Vec<Ident>,
) -> TokenStream {
    let rname = format_ident!("Mysql{}", name);
    let where_fields: Vec<_> = conds
        .iter()
        .map(|cond| {
            let ident = format_ident!("get_{}", cond);
            quote!(self.#ident())
        })
        .collect();
    let insert_fields: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let ident = format_ident!("get_{}", field);
            if customs[index] == 2 {
                quote!(
                    self.#ident().write_to_bytes()?
                )
            } else {
                quote!(self.#ident())
            }
        })
        .collect();
    let update_fields: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let ident = format_ident!("get_{}", field);
            if customs[index] == 2 {
                quote!(
                    self.#ident().write_to_bytes()?,
                )
            } else if customs[index] == 0 {
                quote!(self.#ident(),)
            } else {
                quote!()
            }
        })
        .collect();
    let select_fields: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            if customs[index] == 2 {
                let ident = format_ident!("mut_{}", field);
                quote!(
                    self.#ident().merge_from_bytes(data.#field.as_slice())?
                )
            } else {
                let ident = format_ident!("set_{}", field);
                quote!(self.#ident(data.#field))
            }
        })
        .collect();
    quote! {
        #[derive(FromRow)]
        struct #rname {
            #(#fields:#field_types,)*
        }

        impl DataBackend for #name {
            type Connection = mysql::PooledConn;
            type Error = Error;
            fn patch_table(conn:&mut mysql::PooledConn, exec:bool, database:Option<&str>) -> Result<Vec<String>, Error> {
                let mut new_table = Table::default();
                new_table.set_engine("InnoDb");
                new_table.set_charset("utf8mb4");
                new_table.set_name(#table_name);
                #(
                    #columns
                    new_table.columns.push(column);
                )*
                #(
                    #indexes
                )*
                let old_table = Table::new(database, #table_name, conn)?;
                let diff_sqls = new_table.diff(&old_table)?;
                if exec {
                    for sql in &diff_sqls {
                        conn.exec_drop(sql, Params::Empty)?;
                    }
                }
                Ok(diff_sqls)
            }

            fn select(&mut self, conn:&mut mysql::PooledConn) -> Result<bool, Error> {
                let data:Option<#rname> = conn.exec_first(#select, (#(#where_fields,)*))?;
                if let Some(data) = data {
                    #(#select_fields;)*
                    Ok(true)
                } else {
                    Ok(false)
                }
            }

            fn insert(&mut self, conn:&mut mysql::PooledConn) -> Result<bool, Error> {
                self.mask_all(true);
                let result = conn.exec_iter(#insert, (#(#insert_fields,)*))?;
                Ok(result.affected_rows() == 1)
            }

            fn update(&mut self, conn:&mut mysql::PooledConn) -> Result<bool, Error> {
                self.mask_all(true);
                let result = conn.exec_iter(#update, (#(#update_fields)*#(#where_fields,)*))?;
                Ok(result.affected_rows() == 1)
            }

            fn delete(self, conn:&mut mysql::PooledConn) -> Result<bool, Error> {
                let result = conn.exec_iter(#delete, (#(#where_fields,)*))?;
                Ok(result.affected_rows() == 1)
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
                self.data.clear_mask(true);
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
                self.data.clear_mask(true);
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

pub fn gen_data_mask(configs: &Vec<(PathBuf, ConfigFile)>) -> Vec<TokenStream> {
    let all_dirs = vec![
        SyncDirection::Team,
        SyncDirection::Database,
        SyncDirection::Around,
        SyncDirection::Client,
    ];
    let mut dm_codes = Vec::new();
    for (f, cf) in configs {
        let mod_name = format_ident!("{}", f.file_stem().unwrap().to_str().unwrap());
        for c in &cf.configs {
            let mut client_mask = 0u64;
            let mut around_mask = 0u64;
            let mut database_mask = 0u64;
            let mut team_mask = 0u64;
            let mut single_numbers = Vec::new();
            let mut single_names = Vec::new();
            let mut map_numbers = Vec::new();
            let mut map_names = Vec::new();

            let vname = c.name.clone();
            let name = format_ident!("{}", c.name);

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
            }

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
    dm_codes
}

pub fn gen_data_backend(
    configs: &Vec<(PathBuf, ConfigFile)>,
) -> Result<Vec<TokenStream>, std::fmt::Error> {
    let all_dirs = vec![
        SyncDirection::Team,
        SyncDirection::Database,
        SyncDirection::Around,
        SyncDirection::Client,
    ];
    let mut backend_codes = Vec::new();

    for (_, cf) in configs {
        for c in &cf.configs {
            if c.hide.is_some()
                || !c.fields.iter().any(|field| {
                    field
                        .dirs
                        .as_ref()
                        .unwrap_or(&all_dirs)
                        .contains(&SyncDirection::Database)
                })
            {
                continue;
            }

            let mut columns = Vec::new();
            let mut customs = Vec::new();
            let mut fields = Vec::new();
            let mut rust_field_types = Vec::new();

            let vname = c.name.clone();
            let table_name = vname.to_case(Case::Snake);
            let name = format_ident!("{}", c.name);

            let mut select = BytesMut::new();
            let mut insert = BytesMut::new();
            let mut update = BytesMut::new();
            let mut delete = BytesMut::new();
            write!(select, "SELECT ")?;
            write!(insert, "INSERT INTO `{}` SET ", table_name)?;
            write!(update, "UPDATE `{}` SET ", table_name)?;
            write!(
                delete,
                "DELETE FROM `{}` WHERE {}",
                table_name,
                c.get_primary_cond()?
            )?;

            for f in &c.fields {
                if !f
                    .dirs
                    .as_ref()
                    .unwrap_or(&all_dirs)
                    .contains(&SyncDirection::Database)
                {
                    continue;
                }

                let field = &f.name;
                let field_type = f.r#type.to_db_type();

                fields.push(format_ident!("{}", field));
                rust_field_types.push(f.r#type.to_rust_type());
                if c.is_primary_field(f.name.as_str()) {
                    customs.push(1);
                } else {
                    write!(update, " `{}` = ?,", field)?;
                    if matches!(f.r#type, DataType::Custom { .. }) {
                        customs.push(2);
                    } else {
                        customs.push(0);
                    }
                }
                write!(select, " `{}`,", field)?;
                write!(insert, " `{}` = ?,", field)?;
                let column = quote!(
                    let mut column = Column::default();
                    column.field = #field.into();
                    column.field_type = #field_type.into();
                    column.default = None;
                    column.null = BoolValue::No;
                );
                columns.push(column);
            }
            let mut indexes = Vec::new();
            for (index_type, index) in c.indexes.as_ref().unwrap() {
                let name = match index_type {
                    IndexType::Primary => quote!(None),
                    IndexType::Index(name) => quote!(Some(#name.into())),
                };
                let columns = &index.columns;
                let columns = quote!(vec![#(#columns.into(),)*]);
                let desc = index.desc.is_some() && index.desc.unwrap();
                let unique = index.unique.is_some() && index.unique.unwrap();
                let code = quote!(new_table.add_index(#name, #columns.as_slice(), #desc, #unique););
                indexes.push(code);
            }
            select.truncate(select.len() - 1);
            insert.truncate(insert.len() - 1);
            update.truncate(update.len() - 1);
            write!(
                select,
                " FROM `{}` WHERE {}",
                table_name,
                c.get_primary_cond()?
            )?;
            write!(update, " WHERE {}", c.get_primary_cond()?)?;

            let conds: Vec<_> = c
                .get_primary_fields()
                .iter()
                .map(|field| format_ident!("{}", field))
                .collect();
            let select = unsafe { String::from_utf8_unchecked(select.to_vec()) };
            let insert = unsafe { String::from_utf8_unchecked(insert.to_vec()) };
            let update = unsafe { String::from_utf8_unchecked(update.to_vec()) };
            let delete = unsafe { String::from_utf8_unchecked(delete.to_vec()) };

            let backend_code = gen_backend_code(
                &name,
                &table_name,
                &select,
                &insert,
                &update,
                &delete,
                &columns,
                &indexes,
                &fields,
                &rust_field_types,
                &customs,
                &conds,
            );
            backend_codes.push(backend_code);
        }
    }
    Ok(backend_codes)
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
    let mut ns = Vec::new();
    let mut cmds = Vec::new();
    let mut vnames = Vec::new();

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
        }
    }
    let dm_codes = gen_data_mask(&configs);
    let backend_codes = gen_data_backend(&configs)?;
    let dataset_type_code = gen_dataset_type();

    let data = quote!(
            #![allow(unused_imports)]
            #(mod #mods;)*

            use byteorder::{BigEndian, ByteOrder};
            use dataproxy::{BoolValue, Column, Index, Table};
            use derive_more::From;
            use ecs_engine::{
                CommitChangeSystem, DataBackend, DataSet, FromRow, GameDispatcherBuilder, SceneSyncBackend,
                SyncDirection,
            };
            use mysql::{prelude::Queryable, Params};
            pub use player::Bag;
            use protobuf::{Mask, MaskSet, Message};
            use specs::{
                Component, DefaultVecStorage, FlaggedStorage, HashMapStorage, NullStorage, Tracked, VecStorage,
                World, WorldExt,
            };
            use std::{
                any::Any,
                ops::{Deref, DerefMut},
            };
            #(pub use #inners;)*

            #dataset_type_code

            #(
                impl Component for Type<#files::#names, #ns, #cmds> {
                    type Storage = #storages;
                }

                pub type #names = Type<#files::#names, #ns, #cmds>;
            )*

            #position_code
            #scene_data_code

            pub trait DirectionMask {
                fn mask_by_direction(&self, direction: SyncDirection, ms: &mut MaskSet);
            }
            #(#dm_codes)*

            #[derive(From, Debug)]
            pub enum Error {
                Mysql(mysql::Error),
                Format(std::fmt::Error),
                Protobuf(protobuf::ProtobufError),
            }

            #(#backend_codes)*


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
