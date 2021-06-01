use std::alloc::System;
use std::collections::HashMap;

use convert_case::{Case, Casing};
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::{format_ident, quote, quote_spanned};
use syn::{parse_macro_input, Attribute, Generics, Signature, Visibility};
use syn::{FnArg, ItemFn, Lit, LitStr, Meta, NestedMeta, Type};

lazy_static::lazy_static! {
    static ref COMPONENT_INDEX_MAP: HashMap<String, usize> = {
        HashMap::with_capacity(1024).into()
    };
}

#[allow(mutable_transmutes)]
fn get_component_index(name: &String) -> usize {
    unsafe {
        let map: &HashMap<String, usize> = &COMPONENT_INDEX_MAP;
        let map: &mut HashMap<String, usize> = std::mem::transmute(map);
        if let Some(index) = map.get(name) {
            *index
        } else {
            let index = map.len();
            map.insert(name.clone(), index);
            index
        }
    }
}

fn component_exists(name: &String) -> bool {
    COMPONENT_INDEX_MAP.contains_key(name)
}

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("system types must be one of `single`, `double` or `multiple`")]
    UnexpectedSystemType(Span),
    #[error("duplicate system name")]
    DuplicateSystemName,
    #[error("duplicate system type")]
    DuplicateSystemType,
    #[error("invalid key")]
    InvalidKey(Span),
    #[error("invalid meta found in dynamic")]
    InvalidMetaForDynamic(Span),
    #[error("duplicate dynamic library name")]
    DuplicateDynamicLibraryName,
    #[error("duplicate dynamic function name")]
    DuplicateDynamicFunctionName,
    #[error("static and dynamic can not appear in both time")]
    StaticConflictsDynamic,
    #[error("no path allowed in dynamic")]
    UnexpectedPathInDynamic,
    #[error("method not found for system, use function instead")]
    SelfNotAllowed,
    #[error("system function parameters must be component references, state references or resource references")]
    InvalidArgument(Span),
    #[error(
        "system function parameters must be one of input, component, state and resource, no more no less"
    )]
    ConflictParameterAttribute,
}

impl Error {
    fn span(&self) -> Span {
        match self {
            Error::UnexpectedSystemType(span) => *span,
            Error::InvalidKey(span) => *span,
            Error::InvalidArgument(span) => *span,
            Error::InvalidMetaForDynamic(span) => *span,
            _ => Span::call_site(),
        }
    }

    fn emit(&self) -> proc_macro2::TokenStream {
        let message = format!("{}", self);
        quote_spanned!(self.span() => compile_error!(#message);)
    }
}

#[derive(Copy, Clone, PartialEq)]
enum SystemType {
    Single,
    Double,
    Multiple,
}

#[derive(Default)]
struct SystemAttr {
    system_name: Option<Lit>,
    system_type: Option<SystemType>,
}

impl SystemAttr {
    fn new(system_name: Option<Lit>, system_type: Option<SystemType>) -> Self {
        Self {
            system_type,
            system_name,
        }
    }

    fn parse_meta(meta: &Meta) -> Result<Self, Error> {
        let result = match meta {
            Meta::Path(path) => {
                let ident = path.get_ident().expect("expected system type");
                if ident == "single" {
                    Self::new(None, Some(SystemType::Single))
                } else if ident == "double" {
                    Self::new(None, Some(SystemType::Double))
                } else if ident == "multiple" {
                    Self::new(None, Some(SystemType::Multiple))
                } else {
                    return Err(Error::UnexpectedSystemType(ident.span()));
                }
            }
            Meta::List(items) => {
                let mut n = None;
                let mut s = None;
                for item in &items.nested {
                    let Self {
                        system_name,
                        system_type,
                    } = match item {
                        syn::NestedMeta::Meta(meta) => Self::parse_meta(&meta)?,
                        syn::NestedMeta::Lit(_) => panic!("unexpected literal"),
                    };
                    if let Some(system_name) = system_name {
                        if n.replace(system_name).is_some() {
                            return Err(Error::DuplicateSystemName);
                        }
                    }
                    if let Some(system_type) = system_type {
                        if s.replace(system_type).is_some() {
                            return Err(Error::DuplicateSystemType);
                        }
                    }
                }
                Self::new(n, s)
            }
            Meta::NameValue(name_value) => match name_value.path.get_ident() {
                Some(ident) if ident == "name" => Self::new(Some(name_value.lit.clone()), None),
                Some(ident) => return Err(Error::InvalidKey(ident.span())),
                _ => return Err(Error::InvalidKey(Span::call_site())),
            },
        };
        Ok(result)
    }
}

struct Config {
    attr: SystemAttr,
    visibility: Visibility,
    dynamic: bool,
    lib_name: Option<Lit>,
    func_name: Option<Lit>,
    signature: Sig,
}

fn lit_to_ident(lit: &Lit) -> Ident {
    let (name, span) = match lit {
        Lit::Str(name) => (name.value(), name.span()),
        Lit::Char(name) => (name.value().to_string(), name.span()),
        _ => panic!("invalid system name"),
    };
    Ident::new(&name, span)
}

fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Path(path) => {
            let len = path.path.segments.len();
            path.path.segments[len - 1].ident.to_string()
        }
        _ => panic!("not supported type"),
    }
}

impl Config {
    fn parse(attr: SystemAttr, item: &mut ItemFn) -> Result<Self, Error> {
        let mut to_remove = Vec::new();
        let mut dynamic = false;
        let mut fstatic = false;
        let mut lib_name = None;
        let mut func_name = None;
        for (i, attribute) in item.attrs.iter().enumerate() {
            if let Some(ident) = attribute.path.get_ident() {
                if ident == "static" {
                    to_remove.push(i);
                    fstatic = true;
                }
                if ident == "dynamic" {
                    dynamic = true;
                    to_remove.push(i);
                    let meta = attribute
                        .parse_meta()
                        .map_err(|err| Error::InvalidMetaForDynamic(ident.span()))?;
                    let (l, f) = Self::parse_dynamic_meta(&meta)?;
                    if let Some(l) = l {
                        if lib_name.replace(l).is_some() {
                            return Err(Error::DuplicateDynamicLibraryName);
                        }
                    }
                    if let Some(f) = f {
                        if func_name.replace(f).is_some() {
                            return Err(Error::DuplicateDynamicFunctionName);
                        }
                    }
                }
            }
        }
        if dynamic && fstatic {
            return Err(Error::StaticConflictsDynamic);
        }

        for i in to_remove {
            item.attrs.remove(i);
        }

        let signature = Sig::parse(&mut item.sig)?;

        Ok(Self {
            attr,
            visibility: item.vis.clone(),
            dynamic,
            lib_name,
            func_name,
            signature,
        })
    }

    fn parse_dynamic_meta(meta: &Meta) -> Result<(Option<Lit>, Option<Lit>), Error> {
        let result = match meta {
            Meta::Path(path) => {
                let lit = if path.segments.len() > 0 {
                    let ident = &path.segments[0].ident;
                    let lit = Lit::Str(LitStr::new(ident.to_string().as_str(), ident.span()));
                    Some(lit)
                } else {
                    None
                };
                (lit, None)
            }
            Meta::List(items) => {
                let mut lib_name = None;
                let mut func_name = None;
                for item in &items.nested {
                    let (l, f) = match item {
                        syn::NestedMeta::Meta(meta) => Self::parse_dynamic_meta(meta)?,
                        syn::NestedMeta::Lit(_) => panic!("unexpected literal"),
                    };
                    if let Some(l) = l {
                        if lib_name.replace(l).is_some() {
                            return Err(Error::DuplicateDynamicLibraryName);
                        }
                    }
                    if let Some(f) = f {
                        if func_name.replace(f).is_some() {
                            return Err(Error::DuplicateDynamicFunctionName);
                        }
                    }
                }
                (lib_name, func_name)
            }
            Meta::NameValue(name_value) => match name_value.path.get_ident() {
                Some(ident) if ident == "lib" => (Some(name_value.lit.clone()), None),
                Some(ident) if ident == "func" => (None, Some(name_value.lit.clone())),
                Some(ident) => return Err(Error::InvalidKey(ident.span())),
                _ => return Err(Error::InvalidKey(Span::call_site())),
            },
        };
        Ok(result)
    }

    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }

    fn generate(&self) -> Result<proc_macro2::TokenStream, Error> {
        self.validate()?;

        let system_name = if let Some(system_name) = &self.attr.system_name {
            lit_to_ident(system_name)
        } else {
            format_ident!(
                "{}System",
                self.signature.ident.to_string().to_case(Case::UpperCamel)
            )
        };
        eprintln!("system_name");

        let lib_name = if let Some(lib_name) = &self.lib_name {
            lib_name.clone()
        } else {
            Lit::Str(LitStr::new(
                self.signature.ident.to_string().as_str(),
                self.signature.ident.span(),
            ))
        };
        eprintln!("lib_name");

        let func_name = if let Some(func_name) = &self.func_name {
            func_name.clone()
        } else {
            Lit::Str(LitStr::new(
                self.signature.ident.to_string().as_str(),
                self.signature.ident.span(),
            ))
        };
        eprintln!("func_name");

        let mut components = Vec::new();
        let mut new_components = Vec::new();
        let mut new_indexes = Vec::new();
        let mut new_mutable_names = Vec::new();
        let mut new_index_names = Vec::new();
        for param in &self.signature.parameters {
            match param {
                Parameter::Component(index, mutable) => {
                    let ty = self.signature.component_args[*index].clone();
                    let name = type_to_string(&ty);
                    components.push(format_ident!("{}Mut", name));
                    if !component_exists(&name) {
                        new_indexes.push(get_component_index(&name));
                        new_index_names.push(format_ident!("{}Index", name));
                        new_mutable_names.push(format_ident!("{}Mut", name));
                        new_components.push(format_ident!("{}", name));
                    }
                }
                _ => {}
            }
        }

        eprintln!("quote now");
        let code = quote! {
            #[derive(Default)]
            struct #system_name {
                lib: ecs_engine::DynamicSystem<fn(&UserInfo, &BagInfo)>,
            }

            #(pub const #new_index_names :usize = #new_indexes;)*
            #(pub type #new_mutable_names = ecs_engine::Mutable<#new_components, #new_indexes>;)*

            impl #system_name {
                pub fn setup(mut self, world: &mut specs::World, builder: &mut specs::DispatcherBuilder, dm: &ecs_engine::DynamicManager) {
                    #(world.register::<#components>();)*
                    self.lib.init(#lib_name.into(), #func_name.into(), dm);
                    builder.add(self, #func_name, &[]);
                }
            }

            impl<'a> specs::System<'a> for #system_name {
                type SystemData = (
                    specs::ReadStorage<'a, UserInfoMut>,
                    specs::ReadStorage<'a, BagInfoMut>,
                    specs::Read<'a, ecs_engine::DynamicManager>,
                );

                fn run(&mut self, (user, bag, dm): Self::SystemData) {
                    if let Some(symbol) = self.lib.get_symbol(&dm) {
                        for (user, bag) in (&user, &bag).join() {
                            if let Err(err) = std::panic::catch_unwind(||{
                                (*symbol)(user, bag);
                            }) {
                                log::error!("execute system {} failed with {:?}", #func_name, err);
                            }
                        }
                    } else {
                        log::error!("symbol not found for system {}", #func_name);
                    }
                }
            }
        };
        Ok(code)
    }
}

enum ArgAttr {
    Resource,
    State,
    Input,
    Component,
}

enum Parameter {
    Component(usize, bool),
    Resource(usize, bool),
    State(usize, bool),
    Input,
}

struct Sig {
    ident: Ident,
    generics: Generics,
    parameters: Vec<Parameter>,
    state_args: Vec<Type>,
    resource_args: Vec<Type>,
    component_args: Vec<Type>,
    input: Option<Type>,
}

impl Sig {
    fn parse(item: &mut Signature) -> Result<Self, Error> {
        let mut parameters = Vec::new();
        let mut resource_args = Vec::new();
        let mut state_args = Vec::new();
        let mut component_args = Vec::new();
        let mut input = None;
        for param in &mut item.inputs {
            match param {
                syn::FnArg::Receiver(_) => return Err(Error::SelfNotAllowed),
                syn::FnArg::Typed(arg) => match arg.ty.as_ref() {
                    Type::Reference(ty) => {
                        let mutable = ty.mutability.is_some();
                        let elem = ty.elem.as_ref().clone();
                        let attribute = Self::find_remove_arg_attr(&mut arg.attrs)?;
                        match attribute {
                            Some(ArgAttr::Resource) => {
                                parameters.push(Parameter::Resource(resource_args.len(), mutable));
                                resource_args.push(elem);
                            }
                            Some(ArgAttr::State) => {
                                parameters.push(Parameter::State(state_args.len(), mutable));
                                state_args.push(elem);
                            }
                            Some(ArgAttr::Input) => {
                                parameters.push(Parameter::Input);
                                input.replace(elem);
                            }
                            _ => {
                                parameters
                                    .push(Parameter::Component(component_args.len(), mutable));
                                component_args.push(elem);
                            }
                        }
                    }
                    _ => return Err(Error::InvalidArgument(Span::call_site())),
                },
            }
        }

        Ok(Self {
            ident: item.ident.clone(),
            generics: item.generics.clone(),
            parameters,
            resource_args,
            state_args,
            component_args,
            input,
        })
    }

    fn find_remove_arg_attr(attributes: &mut Vec<Attribute>) -> Result<Option<ArgAttr>, Error> {
        let mut attr = None;
        for i in (0..attributes.len()).rev() {
            match attributes[i].path.get_ident() {
                Some(ident) if ident == "resource" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::Resource).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                Some(ident) if ident == "state" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::State).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                Some(ident) if ident == "input" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::Input).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                Some(ident) if ident == "component" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::Component).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                _ => {}
            }
        }
        Ok(attr)
    }
}

#[proc_macro_attribute]
pub fn system(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemFn);

    let attr = if attr.is_empty() {
        Ok(SystemAttr::default())
    } else {
        let meta = parse_macro_input!(attr as Meta);
        SystemAttr::parse_meta(&meta)
    };

    let result = attr
        .and_then(|attr| Config::parse(attr, &mut input))
        .and_then(|mut config| config.generate());
    let code = match result {
        Ok(code) => code,
        Err(err) => err.emit(),
    };

    TokenStream::from(code)
}
