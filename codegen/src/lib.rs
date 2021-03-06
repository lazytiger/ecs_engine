use proc_macro::TokenStream;
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use convert_case::{Case, Casing};
use proc_macro2::{Ident, Span};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    parse_macro_input, parse_quote, spanned::Spanned, Attribute, FnArg, GenericArgument, ItemFn,
    ItemStruct, Lit, LitBool, LitStr, Meta, Pat, PathArguments, ReturnType, Signature, Type,
    TypePath, Visibility,
};

use generator::parse_config;

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("duplicate output type found")]
    DuplicateOutputType,
    #[error("duplicate component type found")]
    DuplicateComponentType,
    #[error("duplicate resource type found")]
    DuplicateResourceType,
    #[error("duplicate state type found")]
    DuplicateStateType,
    #[error("duplicate storage type found")]
    DuplicateStorageType,
    #[error("WriteStorage should not intersect Component")]
    WriteStorageFoundInComponents,
    #[error("ReadStorage should not intersect with mutable Component")]
    ReadStorageFoundInMutableComponents,
    #[error("invalid key")]
    InvalidKey(Span),
    #[error("invalid meta found in dynamic")]
    InvalidMetaForDynamic(Span),
    #[error("duplicate dynamic library name")]
    DuplicateDynamicLibraryName,
    #[error("duplicate dynamic function name")]
    DuplicateDynamicFunctionName,
    #[error("method not found for system, use function instead")]
    SelfNotAllowed,
    #[error("system function parameters must be component references, state references or resource references")]
    InvalidArgument(Span),
    #[error(
        "system function parameters must be one of input, component, state and resource, no more no less"
    )]
    ConflictParameterAttribute,
    #[error("only one input allowed in system")]
    MultipleInputFound,
    #[error("#[dynamic(\"lib\", \"func\")] is not allowed, use #[dynamic(lib = \"lib\", func = \"func\")] instead")]
    LiteralFoundInDynamicAttribute(Span),
    #[error("Entity type cannot be mutable, remove &mut")]
    EntityCantBeMutable(Span),
    #[error("GameReadStorage type cannot be mutable, remove &mut")]
    ReadStorageCantBeMutable(Span),
    #[error("GameWriteStorage type must be mutable, add &mut")]
    WriteStorageIsNotMutable(Span),
    #[error(
        "invalid return type only Option<Component> or tuple of Option<Component> is accepted"
    )]
    InvalidReturnType(Span),
    #[error("invalid storage type, use GameReadStorage<T> or GameWriteStorage<T>")]
    InvalidStorageType(Span),
}

impl Error {
    fn span(&self) -> Span {
        match self {
            Error::InvalidKey(span) => *span,
            Error::InvalidMetaForDynamic(span) => *span,
            Error::InvalidArgument(span) => *span,
            Error::LiteralFoundInDynamicAttribute(span) => *span,
            Error::EntityCantBeMutable(span) => *span,
            Error::ReadStorageCantBeMutable(span) => *span,
            Error::WriteStorageIsNotMutable(span) => *span,
            Error::InvalidReturnType(span) => *span,
            Error::InvalidStorageType(span) => *span,
            _ => Span::call_site(),
        }
    }

    fn emit(&self) -> proc_macro2::TokenStream {
        let message = format!("{}", self);
        quote_spanned!(self.span() => compile_error!(#message);)
    }
}

#[derive(Default)]
struct SystemAttr {
    system_name: Option<Ident>,
}

impl SystemAttr {
    fn new(system_name: Option<Ident>) -> Self {
        Self { system_name }
    }

    fn parse_meta(meta: &Meta) -> Result<Self, Error> {
        let result = match meta {
            Meta::Path(path) => {
                if let Some(ident) = path.get_ident() {
                    Self::new(Some(ident.clone()))
                } else {
                    Self::new(None)
                }
            }
            _ => Self::new(None),
        };
        Ok(result)
    }
}

struct Config {
    attr: SystemAttr,
    dynamic: bool,
    lib_name: Option<Lit>,
    func_name: Option<Lit>,
    signature: Sig,
}

fn contains_duplicate(data: &Vec<Type>) -> bool {
    if data.len() < 2 {
        false
    } else {
        for i in 0..data.len() {
            let t1 = &data[i];
            for j in (i + 1)..data.len() {
                if t1 == &data[j] {
                    return true;
                }
            }
        }
        false
    }
}

impl Config {
    fn parse(attr: SystemAttr, item: &mut ItemFn) -> Result<Self, Error> {
        let mut to_remove = Vec::new();
        let mut dynamic = true;
        let mut lib_name = None;
        let mut func_name = None;
        for (i, attribute) in item.attrs.iter().enumerate() {
            if let Some(ident) = attribute.path.get_ident() {
                if ident == "dynamic" {
                    to_remove.push(i);
                    let meta = attribute
                        .parse_meta()
                        .map_err(|_err| Error::InvalidMetaForDynamic(ident.span()))?;
                    let (l, f) = Self::parse_dynamic_meta(&meta)?;
                    if let Some(l) = l {
                        if let Lit::Bool(_) = l {
                            dynamic = false;
                        } else {
                            if lib_name.replace(l).is_some() {
                                return Err(Error::DuplicateDynamicLibraryName);
                            }
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

        for i in to_remove {
            item.attrs.remove(i);
        }

        let mut signature = Sig::parse(&mut item.sig)?;
        signature.generate_output_names();

        Ok(Self {
            attr,
            dynamic,
            lib_name,
            func_name,
            signature,
        })
    }

    fn parse_dynamic_meta(meta: &Meta) -> Result<(Option<Lit>, Option<Lit>), Error> {
        let result = match meta {
            Meta::Path(path) => {
                let lit = if path.segments.len() == 1 {
                    let ident = &path.segments[0].ident;
                    let lit = if ident.to_string() == "false" {
                        Lit::Bool(LitBool::new(false, ident.span()))
                    } else {
                        Lit::Str(LitStr::new(ident.to_string().as_str(), ident.span()))
                    };
                    Some(lit)
                } else if path.segments.len() > 1 {
                    return Err(Error::InvalidMetaForDynamic(path.span()));
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
                        syn::NestedMeta::Lit(_) => {
                            return Err(Error::LiteralFoundInDynamicAttribute(meta.span()));
                        }
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
        if contains_duplicate(&self.signature.outputs) {
            return Err(Error::DuplicateOutputType);
        }
        if contains_duplicate(&self.signature.component_args) {
            return Err(Error::DuplicateComponentType);
        }
        if contains_duplicate(&self.signature.resource_args) {
            return Err(Error::DuplicateResourceType);
        }
        if contains_duplicate(&self.signature.state_args) {
            return Err(Error::DuplicateStateType);
        }
        if contains_duplicate(&self.signature.storage_args) {
            return Err(Error::DuplicateStorageType);
        }
        let mut components = self.signature.component_args.clone();
        components.extend(self.signature.outputs.clone().into_iter());
        if contains_duplicate(&components) {
            return Err(Error::DuplicateComponentType);
        }
        let mut components = self.signature.component_args.clone();
        components.extend(self.signature.parameters.iter().filter_map(|param| {
            if let Parameter::Storage(_, index, mutable) = param {
                if *mutable {
                    return Some(self.signature.storage_args[*index].clone());
                }
            }
            None
        }));
        if contains_duplicate(&components) {
            return Err(Error::WriteStorageFoundInComponents);
        }
        let mut components = self.signature.storage_args.clone();
        components.extend(self.signature.parameters.iter().filter_map(|param| {
            if let Parameter::Component(_, index, mutable) = param {
                if *mutable {
                    return Some(self.signature.component_args[*index].clone());
                }
            }
            None
        }));
        if contains_duplicate(&components) {
            return Err(Error::ReadStorageFoundInMutableComponents);
        }
        Ok(())
    }

    fn generate(&self, input: ItemFn) -> Result<proc_macro2::TokenStream, Error> {
        self.validate()?;

        let system_name = if let Some(system_name) = &self.attr.system_name {
            system_name.clone()
        } else {
            format_ident!(
                "{}System",
                self.signature.ident.to_string().to_case(Case::UpperCamel)
            )
        };
        add_system(system_name.to_string());
        let system_fn = format_ident!("{}Fn", system_name);

        let lib_name = if let Some(lib_name) = &self.lib_name {
            lib_name.clone()
        } else {
            Lit::Str(LitStr::new(
                self.signature.ident.to_string().as_str(),
                self.signature.ident.span(),
            ))
        };

        let func_name = if let Some(func_name) = &self.func_name {
            func_name.clone()
        } else {
            Lit::Str(LitStr::new(
                self.signature.ident.to_string().as_str(),
                self.signature.ident.span(),
            ))
        };

        let mut system_sname =
            self.signature
                .parameters
                .iter()
                .fold(Ok(String::new()), |name, param| {
                    if let Parameter::Component(_, index, _) = param {
                        let type_name = type_to_string(&self.signature.component_args[*index]);
                        if is_input_string(&type_name) {
                            let name = name?;
                            return if name.is_empty() {
                                Ok(type_name)
                            } else {
                                Err(Error::MultipleInputFound)
                            };
                        }
                    }
                    name
                })?;
        let mut system_deps = quote!(&[]);
        if system_sname.is_empty() {
            system_sname = quote!(#system_name).to_string();
        } else {
            let name = system_sname.to_case(Case::Snake);
            let dep = format!("{}_input", name);
            system_sname = format!("{}_exec", name);
            system_deps = quote!(&[#dep]);
        }

        // all components should be registered
        let mut component_types = Vec::new();
        // field names
        let mut state_names = Vec::new();
        // field types
        let mut state_types = Vec::new();
        // SystemData types
        let mut system_data_types = Vec::new();
        // function input types
        let mut fn_input_types = Vec::new();
        // function output types
        let mut fn_output_types = Vec::new();
        // function output names
        let mut output_vnames = Vec::new();
        // storage names for output types
        let mut output_snames = Vec::new();
        // vectors for storing output entities.
        let mut output_enames = Vec::new();
        // names for all input parameters
        let mut input_names = Vec::new();
        // names for all function input parameters
        let mut func_names = Vec::new();
        // names for join
        let mut join_names = Vec::new();
        // names for foreach
        let mut foreach_names = Vec::new();
        // names for storing input entities.
        let mut write_components = Vec::new();
        // alias names for storage types.
        let mut input_alias = Vec::new();

        for param in &self.signature.parameters {
            match param {
                Parameter::Component(vname, index, mutable) => {
                    let ty = self.signature.component_args[*index].clone();
                    component_types.push(ty.clone());
                    func_names.push(quote!(#vname));
                    let jname = format_ident!("j{}", vname);
                    foreach_names.push(vname.clone());
                    if *mutable {
                        join_names.push(quote!(&mut #jname));
                        let data = quote!(::specs::WriteStorage<'a, #ty>);
                        system_data_types.push(data);
                        input_names.push(quote!(mut #jname));
                        fn_input_types.push(quote!(&mut #ty));
                        write_components.push(ty);
                    } else {
                        if self.signature.storage_args.contains(&ty) {
                            join_names.push(quote!(#jname));
                        } else {
                            join_names.push(quote!(&#jname));
                        }
                        fn_input_types.push(quote!(&#ty));
                        let data = quote!(::specs::ReadStorage<'a, #ty>);
                        system_data_types.push(data);
                        input_names.push(quote!(#jname));
                    }
                }
                Parameter::State(vname, index, mutable) => {
                    let ty = self.signature.state_args[*index].clone();
                    state_names.push(vname.clone());
                    state_types.push(ty.clone());
                    if *mutable {
                        func_names.push(quote!(&mut self.#vname));
                        fn_input_types.push(quote!(&mut #ty));
                    } else {
                        func_names.push(quote!(&self.#vname));
                        fn_input_types.push(quote!(&#ty));
                    }
                }
                Parameter::Resource(vname, index, mutable, expect) => {
                    let ty = self.signature.resource_args[*index].clone();
                    let data = if *mutable {
                        if *expect {
                            quote!(::specs::WriteExpect<'a, #ty>)
                        } else {
                            quote!(::specs::Write<'a, #ty>)
                        }
                    } else {
                        if *expect {
                            quote!(::specs::ReadExpect<'a, #ty>)
                        } else {
                            quote!(::specs::Read<'a, #ty>)
                        }
                    };
                    system_data_types.push(data);
                    if *mutable {
                        func_names.push(quote!(&mut #vname));
                        input_names.push(quote!(mut #vname));
                        fn_input_types.push(quote!(&mut #ty));
                    } else {
                        func_names.push(quote!(&#vname));
                        input_names.push(quote!(#vname));
                        fn_input_types.push(quote!(&#ty));
                    }
                }
                Parameter::Entity => {
                    let vname = format_ident!("entity");
                    let jname = format_ident!("j{}", vname);
                    input_names.push(quote!(#jname));
                    fn_input_types.push(quote!(&::specs::Entity));
                    system_data_types.push(quote!(::specs::Entities<'a>));
                    foreach_names.push(vname.clone());
                    if self.signature.parameters.iter().any(|param| {
                        if let Parameter::Entities = param {
                            true
                        } else {
                            false
                        }
                    }) {
                        join_names.push(quote!(#jname));
                    } else {
                        join_names.push(quote!(&#jname));
                    }
                    func_names.push(quote!(&#vname));
                }
                Parameter::Entities => {
                    let vname = format_ident!("entity");
                    let jname = format_ident!("jentity");
                    if !self.signature.parameters.iter().any(|param| {
                        if let Parameter::Entity = param {
                            true
                        } else {
                            false
                        }
                    }) {
                        if !self.signature.outputs.is_empty() {
                            foreach_names.push(vname.clone());
                            join_names.push(quote!(#jname));
                        }
                        system_data_types.push(quote!(::specs::Entities<'a>));
                        input_names.push(quote!(#jname));
                    }
                    fn_input_types.push(quote!(&::ecs_engine::GameEntities));
                    input_alias.push(quote!(let #jname:&::ecs_engine::GameEntities = unsafe {std::mem::transmute(&#jname)};));
                    func_names.push(quote!(#jname));
                }
                Parameter::Storage(vname, index, mutable) => {
                    let sname = format_ident!("s{}", vname);
                    let ty = self.signature.storage_args[*index].clone();
                    let storage_type = if *mutable {
                        quote!(&mut ::ecs_engine::GameWriteStorage<#ty>)
                    } else {
                        quote!(& ::ecs_engine::GameReadStorage<#ty>)
                    };
                    fn_input_types.push(quote!(#storage_type));
                    if let Some(name) = self.signature.parameters.iter().find_map(|param| {
                        if let Parameter::Component(name, index, _) = param {
                            if self.signature.component_args[*index] == ty {
                                Some(format_ident!("j{}", name))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }) {
                        input_alias.push(
                            quote!(let #name:#storage_type = unsafe { std::mem::transmute(&#name)};),
                        );
                        func_names.push(quote!(#name));
                    } else {
                        component_types.push(ty.clone());
                        if *mutable {
                            system_data_types.push(quote!(::specs::WriteStorage<'a, #ty>));
                        } else {
                            system_data_types.push(quote!(::specs::ReadStorage<'a, #ty>));
                        }
                        input_names.push(quote!(#sname));
                        input_alias.push(quote!(let #sname:#storage_type = unsafe { std::mem::transmute(&#sname)};));
                        func_names.push(quote!(#sname));
                    }
                }
            }
        }

        for (i, typ) in self.signature.outputs.iter().enumerate() {
            let vname = &self.signature.output_names[i];
            system_data_types.push(quote!(::specs::WriteStorage<'a, #typ>));
            input_names.push(quote!(mut #vname));
            output_snames.push(vname.clone());
            fn_output_types.push(quote!(Option<#typ>));
            output_vnames.push(format_ident!("r{}", i));
            component_types.push(typ.clone());
            join_names.push(quote!(!&#vname));
            foreach_names.push(format_ident!("_"));
            output_enames.push(format_ident!("e{}", vname));
            write_components.push(typ.clone());
        }

        if !self.signature.outputs.is_empty() && !self.signature.has_entities() {
            system_data_types.push(quote!(::specs::Entities<'a>));
            let vname = format_ident!("entity");
            let jname = format_ident!("j{}", vname);
            foreach_names.push(vname);
            input_names.push(quote!(#jname));
            join_names.push(quote!(&#jname));
        }

        let output_code = quote! {
           #(if let Some(#output_vnames) = #output_vnames{
                #output_enames.push((entity, #output_vnames));
            })*
        };

        let (dynamic_init, dynamic_fn, func_call) = if self.dynamic {
            system_data_types.push(quote!(::specs::Read<'a, ::ecs_engine::DynamicManager>));
            state_names.push(format_ident!("lib"));
            state_types.push(parse_quote!(::ecs_engine::DynamicSystem<fn(#(#fn_input_types,)*) -> ::std::option::Option<(#(#fn_output_types),*)>>));
            input_names.push(quote!(dm));
            let dynamic_init = quote!(self.lib.init(#lib_name.into(), #func_name.into(), dm););
            let dynamic_fn =
                quote!(pub type #system_fn = fn(#(#fn_input_types,)*) ->(#(#fn_output_types),*););
            let dynamic_call = quote! {
                if let Some((#(#output_vnames),*)) = {(*symbol)(#(#func_names,)*)} {
                    #output_code
                }
            };
            (dynamic_init, dynamic_fn, dynamic_call)
        } else {
            let symbol = self.signature.ident.clone();
            let static_call = quote! {
                let (#(#output_vnames),*) = #symbol(#(#func_names,)*);
                #output_code
            };
            (quote!(), quote!(), static_call)
        };

        let system_setup = quote! {
            #dynamic_fn

            #[derive(Default)]
            pub struct #system_name {
                #(#state_names:#state_types,)*
            }

            impl #system_name {
                    pub fn setup(mut self, world: &mut ::specs::World, builder: &mut ::ecs_engine::GameDispatcherBuilder, dm: &::ecs_engine::DynamicManager) {
                        #(world.register::<#component_types>();)*
                        #dynamic_init
                        builder.add(self, #system_sname, #system_deps);
                    }
                }
        };

        let system_code = {
            let run_code = quote! {
                (#(#join_names,)*).join().for_each(|(#(#foreach_names,)*)| {
                    #func_call
                });
                #(#output_enames.into_iter().for_each(|(entity, c)|{
                    if let Err(err) = #output_snames.insert(entity, c) {
                        log::error!("insert component failed:{}", err);
                    }
                });)*
            };
            let run_code = if self.dynamic {
                quote! {
                   if let Some(symbol) = self.lib.get_symbol(&dm) {
                        #(#input_alias)*
                        #run_code
                   } else {
                        log::error!("symbol not found for system {}", #func_name);
                    }
                }
            } else {
                quote! {
                    #(#input_alias)*
                    #run_code
                }
            };
            quote! {
                impl<'a> ::specs::System<'a> for #system_name {
                    type SystemData = (
                        #(#system_data_types,)*
                    );

                    fn run(&mut self, (#(#input_names,)*): Self::SystemData) {
                        #(let mut #output_enames = Vec::new();)*
                        #run_code
                    }
                }
            }
        };

        let func_code = if self.dynamic {
            quote!()
        } else {
            quote!(#input)
        };

        Ok(quote! {
            #system_setup
            #system_code
            #func_code
        })
    }
}

enum ArgAttr {
    Resource(bool),
    State,
}

enum Parameter {
    Component(Ident, usize, bool),
    Resource(Ident, usize, bool, bool),
    State(Ident, usize, bool),
    Storage(Ident, usize, bool),
    Entity,
    Entities,
}

struct Sig {
    ident: Ident,
    parameters: Vec<Parameter>,
    state_args: Vec<Type>,
    resource_args: Vec<Type>,
    storage_args: Vec<Type>,
    component_args: Vec<Type>,
    outputs: Vec<Type>,
    output_names: Vec<Ident>,
}

impl Sig {
    fn has_entities(&self) -> bool {
        self.parameters.iter().any(|param| match param {
            Parameter::Entity => true,
            Parameter::Entities => true,
            _ => false,
        })
    }

    fn parse(item: &mut Signature) -> Result<Self, Error> {
        let mut parameters = Vec::new();
        let mut resource_args = Vec::new();
        let mut storage_args = Vec::new();
        let mut state_args = Vec::new();
        let mut component_args = Vec::new();
        let mut index = 0usize;
        for param in &mut item.inputs {
            index += 1;
            match param {
                syn::FnArg::Receiver(_) => return Err(Error::SelfNotAllowed),
                syn::FnArg::Typed(arg) => {
                    let name = format_ident!("i{}", index);
                    match arg.ty.as_ref() {
                        Type::Reference(ty) => {
                            let mutable = ty.mutability.is_some();
                            let elem = ty.elem.as_ref();
                            let attribute = Self::find_remove_arg_attr(&mut arg.attrs)?;
                            match attribute {
                                Some(ArgAttr::Resource(expect)) => {
                                    parameters.push(Parameter::Resource(
                                        name,
                                        resource_args.len(),
                                        mutable,
                                        expect,
                                    ));
                                    resource_args.push(elem.clone());
                                }
                                Some(ArgAttr::State) => {
                                    parameters.push(Parameter::State(
                                        name,
                                        state_args.len(),
                                        mutable,
                                    ));
                                    state_args.push(elem.clone())
                                }
                                _ => {
                                    if is_storage(elem) {
                                        if mutable && is_read_storage(elem) {
                                            return Err(Error::ReadStorageCantBeMutable(
                                                arg.span(),
                                            ));
                                        }
                                        if !mutable && is_write_storage(elem) {
                                            return Err(Error::WriteStorageIsNotMutable(
                                                arg.span(),
                                            ));
                                        }
                                        let ctype = get_storage_type(elem)?;
                                        parameters.push(Parameter::Storage(
                                            name,
                                            storage_args.len(),
                                            mutable,
                                        ));
                                        storage_args.push(ctype);
                                    } else if is_entity(elem) {
                                        if mutable {
                                            return Err(Error::EntityCantBeMutable(arg.span()));
                                        }
                                        parameters.push(Parameter::Entity);
                                    } else if is_entities(elem) {
                                        if mutable {
                                            return Err(Error::EntityCantBeMutable(arg.span()));
                                        }
                                        parameters.push(Parameter::Entities);
                                    } else {
                                        parameters.push(Parameter::Component(
                                            name,
                                            component_args.len(),
                                            mutable,
                                        ));
                                        component_args.push(elem.clone());
                                    }
                                }
                            }
                        }
                        _ => return Err(Error::InvalidArgument(Span::call_site())),
                    }
                }
            }
        }

        let mut outputs = Vec::new();
        match &item.output {
            ReturnType::Default => {}
            ReturnType::Type(_, ty) => match ty.as_ref() {
                Type::Path(path) => {
                    if !is_type(ty, &["Option"]) {
                        return Err(Error::InvalidReturnType(item.output.span()));
                    }
                    let typ = get_option_inner_type(path)?;
                    outputs.push(typ);
                }
                Type::Tuple(tuple) => {
                    for elem in &tuple.elems {
                        if !is_type(elem, &["Option"]) {
                            return Err(Error::InvalidReturnType(item.output.span()));
                        }
                        match elem {
                            Type::Path(path) => {
                                let typ = get_option_inner_type(path)?;
                                outputs.push(typ);
                            }
                            _ => return Err(Error::InvalidReturnType(elem.span())),
                        }
                    }
                }
                _ => return Err(Error::InvalidReturnType(item.output.span())),
            },
        }

        Ok(Self {
            ident: item.ident.clone(),
            parameters,
            resource_args,
            state_args,
            component_args,
            storage_args,
            outputs,
            output_names: Vec::default(),
        })
    }

    fn generate_output_names(&mut self) {
        let mut index = 0usize;
        for typ in &self.outputs {
            index += 1;
            if let Some(name) = self.get_input_component_name(typ) {
                self.output_names.push(name);
            } else {
                self.output_names.push(format_ident!("o{}", index));
            }
        }
    }

    fn get_input_component_name(&self, typ: &Type) -> Option<Ident> {
        self.parameters
            .iter()
            .find(|param| match param {
                Parameter::Component(_, index, _) => {
                    let ty = &self.component_args[*index];
                    if ty == typ {
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            })
            .map(|param| match param {
                Parameter::Component(ident, _, _) => ident.clone(),
                _ => unreachable!(),
            })
    }

    fn find_remove_arg_attr(attributes: &mut Vec<Attribute>) -> Result<Option<ArgAttr>, Error> {
        let mut attr = None;
        for i in (0..attributes.len()).rev() {
            match attributes[i].path.get_ident() {
                Some(ident) if ident == "resource" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::Resource(false)).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                Some(ident) if ident == "expect" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::Resource(true)).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                Some(ident) if ident == "state" => {
                    attributes.remove(i);
                    if attr.replace(ArgAttr::State).is_some() {
                        return Err(Error::ConflictParameterAttribute);
                    }
                }
                _ => {}
            }
        }
        Ok(attr)
    }
}

fn get_option_inner_type(path: &TypePath) -> Result<Type, Error> {
    let segment = &path.path.segments[0];
    match &segment.arguments {
        PathArguments::AngleBracketed(bracketed) => {
            let arg = bracketed.args.iter().next().unwrap();
            match arg {
                GenericArgument::Type(ty) => Ok(ty.clone()),
                _ => Err(Error::InvalidReturnType(path.span())),
            }
        }
        _ => {
            return Err(Error::InvalidReturnType(path.span()));
        }
    }
}

fn is_type(ty: &Type, segments: &[&str]) -> bool {
    if let Type::Path(path) = ty {
        path_match(path, segments)
    } else {
        false
    }
}

fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().split("::").last().unwrap().into()
}

fn path_match(path: &TypePath, segments: &[&str]) -> bool {
    segments
        .iter()
        .zip(path.path.segments.iter())
        .all(|(a, b)| b.ident == *a)
}

fn is_entity(ty: &Type) -> bool {
    is_type(ty, &["Entity"])
        || is_type(ty, &["specs", "Entity"])
        || is_type(ty, &["specs", "world", "Entity"])
}

fn is_entities(ty: &Type) -> bool {
    is_type(ty, &["GameEntities"]) || is_type(ty, &["ecs_engine", "GameEntities"])
}

fn is_storage(ty: &Type) -> bool {
    is_read_storage(ty) || is_write_storage(ty)
}

fn is_read_storage(ty: &Type) -> bool {
    is_type(ty, &["GameReadStorage"]) || is_type(ty, &["ecs_engine", "GameReadStorage"])
}

fn is_write_storage(ty: &Type) -> bool {
    is_type(ty, &["GameWriteStorage"]) || is_type(ty, &["specs", "GameWriteStorage"])
}

fn get_storage_type(ty: &Type) -> Result<Type, Error> {
    if let Type::Path(path) = ty {
        if let Some(seg) = path.path.segments.last() {
            if let PathArguments::AngleBracketed(args) = &seg.arguments {
                if args.args.len() == 1 {
                    let arg = args.args.last().unwrap();
                    if let GenericArgument::Type(ty) = arg {
                        return Ok(ty.clone());
                    }
                }
            }
        }
    }
    return Err(Error::InvalidStorageType(ty.span()));
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
        .and_then(|config| config.generate(input));
    let code = match result {
        Ok(code) => code,
        Err(err) => err.emit(),
    };

    code.into()
}

#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemFn);
    input.vis = Visibility::Inherited;
    let name = input.sig.ident.clone();
    let pname = format_ident!("__private_{}", name);
    let sname = name.to_string();

    let mut pinput = input.clone();
    let mut call_names = Vec::new();
    let mut input_names = Vec::new();
    let mut input_types = Vec::new();
    for param in &mut pinput.sig.inputs {
        match param {
            FnArg::Typed(pt) => {
                let name = match pt.pat.as_ref() {
                    Pat::Ident(ident) => ident.ident.clone(),
                    _ => unreachable!(),
                };
                call_names.push(name.clone());
                match pt.ty.as_mut() {
                    Type::Reference(_) => {
                        input_names.push(quote!(#name));
                        input_types.push(pt.ty.as_ref().clone());
                    }
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    }

    let return_type = match &input.sig.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => {
            let ty = ty.as_ref().clone();
            quote!(#ty)
        }
    };
    let pinput = quote! {
        fn #name(#(#call_names:#input_types,)*) -> ::std::option::Option<#return_type> {
            match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(||#pname(#(#input_names,)*))) {
                Ok(r) => Some(r),
                Err(err) => {
                    log::error!("call system func {} failed:{:?}", #sname, err);
                    None
                }
            }
        }
    };

    input.sig.ident = pname.clone();
    let fn_check = if attr.is_empty() {
        quote!()
    } else {
        let attr = parse_macro_input!(attr as Meta);
        let fn_type = attr.path().clone();
        let type_name = format_ident!("__FN_{}", name.clone().to_string().to_uppercase());
        quote!(static #type_name:#fn_type = #pname;)
    };

    let code = quote! {
        #[no_mangle]
        extern "C" #pinput
        #input
        #fn_check
    };
    code.into()
}

#[allow(dead_code)]
fn is_primitive(ty: &Type) -> bool {
    for sty in [
        "u8", "u16", "u32", "u64", "u128", "usize", "bool", "char", "f32", "f64", "i8", "i16",
        "i32", "i64", "i128", "isize",
    ] {
        if is_type(ty, &[sty]) {
            return true;
        }
    }
    false
}

#[proc_macro_attribute]
pub fn init_log(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);
    quote!(
        #item

        #[no_mangle]
        extern "C" fn init_logger(param: ::ecs_engine::LogParam) {
            ::ecs_engine::init_logger(param);
        }
    )
    .into()
}

lazy_static::lazy_static! {
    static ref NAMES: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    static ref SYSTEMS: Mutex<Vec<String>> = Mutex::new(Vec::new());
}

fn add_system(name: String) {
    SYSTEMS.lock().unwrap().push(name);
}

fn is_input_string(type_name: &String) -> bool {
    NAMES.lock().unwrap().contains_key(type_name)
}

#[proc_macro_attribute]
pub fn request(attr: TokenStream, item: TokenStream) -> TokenStream {
    let meta = parse_macro_input!(attr as Meta);
    if let Meta::Path(path) = meta {
        let mut config_path = PathBuf::new();
        path.segments
            .iter()
            .for_each(|seg| config_path.push(seg.ident.to_string()));
        match parse_config(config_path) {
            Err(err) => {
                let message = format!("parse request dir failed:{:?}", err);
                return quote!(compile_error!(#message);).into();
            }
            Ok(configs) => {
                let mut names = NAMES.lock().unwrap();
                configs.iter().for_each(|(_, file)| {
                    file.configs.iter().for_each(|config| {
                        if config.hide.is_none() {
                            names.insert(config.name.clone(), false);
                        }
                    })
                })
            }
        }
    }
    item
}

#[proc_macro_attribute]
pub fn setup(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let err_code = quote!(
        compile_error!("invalid setup function, signature should like this `pub fn setup(world:&mut World, builder:&mut GameDispatcherBuilder, dm:&DynamicManager) `");
    );
    let item = parse_macro_input!(item as ItemFn);
    if let Visibility::Inherited = item.vis {
        return err_code.into();
    }
    if item.sig.inputs.len() != 3 {
        return err_code.into();
    }

    for (index, input) in item.sig.inputs.iter().enumerate() {
        match input {
            FnArg::Receiver(_) => {
                return err_code.into();
            }
            FnArg::Typed(typed) => {
                if let Type::Reference(ty) = typed.ty.as_ref() {
                    if !match index {
                        0 => ty.mutability.is_some() && is_type(ty.elem.as_ref(), &["World"]),
                        1 => {
                            ty.mutability.is_some()
                                && is_type(ty.elem.as_ref(), &["GameDispatcherBuilder"])
                        }
                        2 => {
                            ty.mutability.is_none()
                                && is_type(ty.elem.as_ref(), &["DynamicManager"])
                        }
                        _ => unreachable!(),
                    } {
                        return err_code.into();
                    }
                } else {
                    return err_code.into();
                }
            }
        }
    }

    let systems: Vec<_> = SYSTEMS
        .lock()
        .unwrap()
        .iter()
        .map(|name| format_ident!("{}", name))
        .collect();

    quote!(
        pub fn setup(world:&mut World, builder:&mut GameDispatcherBuilder, dm:&DynamicManager)  {
            #(
                #systems::default().setup(world, builder, dm);
            )*
        }
    )
    .into()
}

#[proc_macro_derive(FromRow)]
pub fn derive_from_rows(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemStruct);
    let name = input.ident.clone();
    let row_len = input.fields.len();
    let mut field_names = Vec::new();
    let mut field_indexes = Vec::new();
    for field in &input.fields {
        field_indexes.push(field_names.len());
        field_names.push(field.ident.clone().unwrap());
    }
    quote!(
        impl ::mysql::prelude::FromRow for #name {
            fn from_row_opt(row: ::mysql::Row) -> Result<Self, ::mysql::FromRowError>
            where
                Self: Sized,
            {
                if row.len() != #row_len {
                    Err(::mysql::FromRowError(row))
                } else {
                    Ok(Self {
                        #(
                            #field_names: row.get(#field_indexes).ok_or_else(||::mysql::FromRowError(row.clone()))?,
                        )*
                    })
                }
            }
        }
    ).into()
}
