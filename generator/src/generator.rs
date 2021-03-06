use crate::{
    dataset::gen_dataset, format_file, gen_messages, gen_protos, parse_config,
    request::gen_request, response::gen_response, string_to_u32, ConfigFile, Error,
};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

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
    /// 请求是否需要保持顺序
    keep_order: bool,
    /// 是否丢弃重复请求
    keep_duplicate: bool,
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

    pub fn keep_order(&mut self) -> &mut Self {
        self.keep_order = true;
        self
    }

    pub fn keep_duplicate(&mut self) -> &mut Self {
        self.keep_duplicate = true;
        self
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
        gen_request(
            self.keep_order,
            self.keep_duplicate,
            self.request_dir.clone(),
            self.config_dir.clone(),
            self.proto_dir.clone(),
        )?;
        gen_response(
            self.response_dir.clone(),
            self.config_dir.clone(),
            self.proto_dir.clone(),
        )?;
        gen_dataset(
            self.dataset_dir.clone(),
            self.config_dir.clone(),
            self.proto_dir.clone(),
        )?;
        Ok(())
    }
}

pub fn gen_io_config<F>(
    config_type: &str,
    dir: PathBuf,
    mut config_dir: PathBuf,
    mut proto_dir: PathBuf,
    codegen: F,
) -> Result<(), Error>
where
    F: Fn(
        Vec<(PathBuf, ConfigFile)>,
        Vec<Ident>,
        Vec<Ident>,
        Vec<Ident>,
        Vec<TokenStream>,
        Vec<u32>,
    ) -> Result<String, Error>,
{
    config_dir.push(config_type);
    proto_dir.push(config_type);

    let configs = parse_config(config_dir)?;

    gen_messages(&configs, proto_dir.clone(), false)?;
    gen_protos(proto_dir, dir.clone())?;

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
                cmds.push(string_to_u32(c.name.as_bytes()));
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
    let data = codegen(configs, mods, names, files, inners, cmds)?;

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

    format_file(name)?;
    Ok(())
}
