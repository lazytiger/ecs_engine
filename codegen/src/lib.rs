use proc_macro::TokenStream;
use std::collections::HashMap;

use convert_case::{Case, Casing};
use proc_macro2::{Ident, Span};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    parse_macro_input, parse_quote, spanned::Spanned, Attribute, FnArg, GenericArgument, ItemFn,
    Lit, LitStr, Meta, Pat, PathArguments, ReturnType, Signature, Type, TypePath, Visibility,
};

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
    #[error("duplicate output type found")]
    DuplicateOutputType,
    #[error("duplicate component type found")]
    DuplicateComponentType,
    #[error("duplicate resource type found")]
    DuplicateResourceType,
    #[error("duplicate state type found")]
    DuplicateStateType,
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
    #[error("method not found for system, use function instead")]
    SelfNotAllowed,
    #[error("system function parameters must be component references, state references or resource references")]
    InvalidArgument(Span),
    #[error(
        "system function parameters must be one of input, component, state and resource, no more no less"
    )]
    ConflictParameterAttribute,
    #[error("component type should not use full path name")]
    InvalidComponentType(Span),
    #[error("only one input allowed in system")]
    MultipleInputFound,
    #[error("#[dynamic(\"lib\", \"func\")] is not allowed, use #[dynamic(lib = \"lib\", func = \"func\")] instead")]
    LiteralFoundInDynamicAttribute(Span),
    #[error("invalid literal as identifier")]
    InvalidLiteralFoundForName(Span),
    #[error("Entity type cannot be mutable, remove &mut")]
    EntityCantBeMutable(Span),
    #[error(
        "invalid return type only Option<Component> or tuple of Option<Component> is accepted"
    )]
    InvalidReturnType(Span),
}

impl Error {
    fn span(&self) -> Span {
        match self {
            Error::UnexpectedSystemType(span) => *span,
            Error::InvalidKey(span) => *span,
            Error::InvalidArgument(span) => *span,
            Error::InvalidMetaForDynamic(span) => *span,
            Error::InvalidComponentType(span) => *span,
            Error::InvalidLiteralFoundForName(span) => *span,
            Error::EntityCantBeMutable(span) => *span,
            Error::InvalidReturnType(span) => *span,
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
                        syn::NestedMeta::Lit(_) => {
                            return Err(Error::LiteralFoundInDynamicAttribute(meta.span()))
                        }
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
    dynamic: bool,
    lib_name: Option<Lit>,
    func_name: Option<Lit>,
    signature: Sig,
}

fn lit_to_ident(lit: &Lit) -> Result<Ident, Error> {
    let (name, span) = match lit {
        Lit::Str(name) => (name.value(), name.span()),
        Lit::Char(name) => (name.value().to_string(), name.span()),
        _ => return Err(Error::InvalidLiteralFoundForName(lit.span())),
    };
    Ok(Ident::new(&name, span))
}

fn component_type_to_string(ty: &Type) -> Result<String, Error> {
    match ty {
        Type::Path(path) => {
            if path.path.segments.len() > 1 || path.qself.is_some() {
                Err(Error::InvalidComponentType(ty.span()))
            } else {
                Ok(path.path.segments[0].ident.to_string())
            }
        }
        _ => Err(Error::InvalidComponentType(Span::call_site())),
    }
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
        let mut dynamic = false;
        let mut fstatic = false;
        let mut lib_name = None;
        let mut func_name = None;
        for (i, attribute) in item.attrs.iter().enumerate() {
            if let Some(ident) = attribute.path.get_ident() {
                if ident == "statics" {
                    to_remove.push(i);
                    fstatic = true;
                }
                if ident == "dynamic" {
                    dynamic = true;
                    to_remove.push(i);
                    let meta = attribute
                        .parse_meta()
                        .map_err(|_err| Error::InvalidMetaForDynamic(ident.span()))?;
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
        let mut components = self.signature.component_args.clone();
        self.signature
            .outputs
            .iter()
            .for_each(|ty| components.push(ty.clone()));
        if let Some(t) = &self.signature.input {
            components.push(t.clone());
        }
        if contains_duplicate(&components) {
            return Err(Error::DuplicateComponentType);
        }
        Ok(())
    }

    fn generate(&self, input: ItemFn) -> Result<proc_macro2::TokenStream, Error> {
        self.validate()?;

        let system_name = if let Some(system_name) = &self.attr.system_name {
            lit_to_ident(system_name)?
        } else {
            format_ident!(
                "{}System",
                self.signature.ident.to_string().to_case(Case::UpperCamel)
            )
        };
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

        // all components should be registered
        let mut components = Vec::new();
        // all components had not been defined
        let mut new_components = Vec::new();
        // index for all components had not been defined
        let mut new_indexes = Vec::new();
        // alias name for all components had not been defined
        let mut new_mutable_names = Vec::new();
        // alias index name for all components had not been defined
        let mut new_index_names = Vec::new();
        // field names
        let mut state_names = Vec::new();
        // field types
        let mut states = Vec::new();
        // SystemData types
        let mut system_data = Vec::new();
        // function input types
        let mut fn_inputs = Vec::new();
        // function output types
        let mut fn_outputs = Vec::new();
        // function output names
        let mut output_vnames = Vec::new();
        // Mutable alias types for output type
        let mut output_types = Vec::new();
        // storage names for output types
        let mut output_snames = Vec::new();
        // vector names for output
        let mut output_vectors = Vec::new();
        // names for all input parameters
        let mut input_names = Vec::new();
        // names for all function input parameters
        let mut func_names = Vec::new();
        // names for join
        let mut join_names = Vec::new();
        // names for foreach
        let mut foreach_names = Vec::new();
        let mut input_storage = quote!();

        for param in &self.signature.parameters {
            match param {
                Parameter::Component(vname, index, mutable) => {
                    let ty = self.signature.component_args[*index].clone();
                    let tname = component_type_to_string(&ty)?;
                    let mut_ident = format_ident!("{}Mut", tname);
                    components.push(mut_ident.clone());
                    if !component_exists(&tname) {
                        new_indexes.push(get_component_index(&tname));
                        new_index_names.push(format_ident!("{}Index", tname));
                        new_mutable_names.push(mut_ident.clone());
                        new_components.push(format_ident!("{}", tname));
                    }
                    func_names.push(quote!(#vname));
                    let jname = format_ident!("j{}", vname);
                    foreach_names.push(vname.clone());
                    if *mutable {
                        join_names.push(quote!(&mut #jname));
                        fn_inputs.push(quote!(&mut #mut_ident));
                    } else {
                        join_names.push(quote!(&#jname));
                        fn_inputs.push(quote!(&#ty));
                    }
                    if *mutable || self.signature.is_return_type(&ty) {
                        let data = quote!(::specs::WriteStorage<'a, #mut_ident>);
                        system_data.push(data);
                        input_names.push(quote!(mut #jname));
                    } else {
                        let data = quote!(::specs::ReadStorage<'a, #mut_ident>);
                        system_data.push(data);
                        input_names.push(quote!(#jname));
                    };
                }
                Parameter::State(vname, index, mutable) => {
                    let ty = self.signature.state_args[*index].clone();
                    state_names.push(vname.clone());
                    states.push(ty.clone());
                    if *mutable {
                        func_names.push(quote!(&mut self.#vname));
                        fn_inputs.push(quote!(&mut #ty));
                    } else {
                        func_names.push(quote!(&self.#vname));
                        fn_inputs.push(quote!(&#ty));
                    }
                }
                Parameter::Resource(vname, index, mutable) => {
                    let ty = self.signature.resource_args[*index].clone();
                    let data = if *mutable {
                        quote!(::specs::Write<'a, #ty>)
                    } else {
                        quote!(::specs::Read<'a, #ty>)
                    };
                    system_data.push(data);
                    if *mutable {
                        func_names.push(quote!(&mut #vname));
                        input_names.push(quote!(mut #vname));
                        fn_inputs.push(quote!(&mut #ty));
                    } else {
                        func_names.push(quote!(&#vname));
                        input_names.push(quote!(#vname));
                        fn_inputs.push(quote!(&#ty));
                    }
                }
                Parameter::Input(vname) => {
                    if let Some(ty) = &self.signature.input {
                        let tname = component_type_to_string(ty)?;
                        components.push(format_ident!("{}", tname));
                        fn_inputs.push(quote!(&#ty));
                        let jname = format_ident!("j{}", vname);
                        join_names.push(quote!(&#jname));
                        func_names.push(quote!(&#vname));
                        foreach_names.push(vname.clone());
                        input_names.push(quote!(mut #jname));
                        input_storage = quote!(#jname);
                        system_data.push(quote!(::specs::WriteStorage<'a, #ty>))
                    }
                }
                Parameter::Entity(vname) => {
                    let jname = format_ident!("j{}", vname);
                    join_names.push(quote!(&#jname));
                    input_names.push(quote!(#jname));
                    fn_inputs.push(quote!(&::specs::Entity));
                    system_data.push(quote!(::specs::Entities<'a>));
                    foreach_names.push(vname.clone());
                    func_names.push(quote!(&#vname));
                }
            }
        }

        for (i, typ) in self.signature.outputs.iter().enumerate() {
            let vname = &self.signature.output_names[i];
            let tname = component_type_to_string(&typ)?;
            let mut_ident = format_ident!("{}Mut", tname);
            if vname.to_string().starts_with('o') {
                if !component_exists(&tname) {
                    new_indexes.push(get_component_index(&tname));
                    new_index_names.push(format_ident!("{}Index", tname));
                    new_mutable_names.push(mut_ident.clone());
                    new_components.push(format_ident!("{}", tname));
                }
                system_data.push(quote!(::specs::WriteStorage<'a, #mut_ident>));
                input_names.push(quote!(mut #vname));
                output_snames.push(vname.clone());
            } else {
                output_snames.push(format_ident!("j{}", vname));
            }
            fn_outputs.push(quote!(Option<#typ>));
            output_vnames.push(format_ident!("r{}", i));
            output_vectors.push(format_ident!("v{}", i));
            output_types.push(mut_ident);
        }

        if (!self.signature.outputs.is_empty() || self.signature.input.is_some())
            && !self.signature.has_entity()
        {
            system_data.push(quote!(::specs::Entities<'a>));
            let vname = format_ident!("entity");
            let jname = format_ident!("j{}", vname);
            foreach_names.push(vname);
            input_names.push(quote!(#jname));
            join_names.push(quote!(&#jname));
        }

        let (dynamic_init, dynamic_fn, func_call) = if self.dynamic {
            system_data.push(quote!(::specs::Read<'a, ::ecs_engine::DynamicManager>));
            state_names.push(format_ident!("lib"));
            states.push(parse_quote!(::ecs_engine::DynamicSystem<fn(#(#fn_inputs,)*) -> ::std::thread::Result<(#(#fn_outputs),*)>>));
            input_names.push(quote!(dm));
            let dynamic_init = quote!(self.lib.init(#lib_name.into(), #func_name.into(), dm););
            let dynamic_fn =
                quote!(pub type #system_fn = fn(#(#fn_inputs,)*) ->(#(#fn_outputs),*););
            let dynamic_call = quote! {
                match {(*symbol)(#(#func_names,)*)} {
                    Err(err)  => log::error!("call dynamic function {} panic {:?}", #func_name, err),
                    Ok((#(#output_vnames),*)) => {
                        #(if let Some(#output_vnames) = #output_vnames{
                            #output_vectors.push((entity, #output_vnames));
                        })*
                    }
                }
            };
            (dynamic_init, dynamic_fn, dynamic_call)
        } else {
            let symbol = self.signature.ident.clone();
            let static_call = quote! {
                let (#(#output_vnames),*) = #symbol(#(#func_names,)*);
                #(if let Some(#output_vnames) = #output_vnames{
                    #output_vectors.push((entity, #output_vnames));
                })*
            };
            (quote!(), quote!(), static_call)
        };

        let (purge_init, purge_push, purge_done) = if self.signature.input.is_some() {
            let init = quote!(let mut ev = Vec::new(););
            let push = quote!(ev.push(entity););
            let purge = quote! {
                for e in ev {
                    #input_storage.remove(e);
                }
            };
            (init, push, purge)
        } else {
            (quote!(), quote!(), quote!())
        };

        let system_setup = quote! {
            #dynamic_fn

            #[derive(Default)]
            pub struct #system_name {
                #(#state_names:#states,)*
            }

            #(pub const #new_index_names :usize = #new_indexes;)*
            #(pub type #new_mutable_names = ::ecs_engine::Mutable<#new_components, #new_indexes>;)*
            impl #system_name {
                    pub fn setup(mut self, world: &mut ::specs::World, builder: &mut ::specs::DispatcherBuilder, dm: &::ecs_engine::DynamicManager) {
                        #(world.register::<#components>();)*
                        #dynamic_init
                        builder.add(self, #func_name, &[]);
                    }
                }
        };

        let system_type = if let Some(system_type) = self.attr.system_type {
            system_type
        } else {
            SystemType::Single
        };

        let system_code = match system_type {
            SystemType::Single => {
                let run_code = quote! {
                            #(let mut #output_vectors = Vec::new();)*
                            #purge_init
                            for (#(#foreach_names,)*) in (#(#join_names,)*).join() {
                                #purge_push
                                #func_call
                            }
                            #(
                                for (entity, #output_vnames) in #output_vectors {
                                    if let Err(err) = #output_snames.insert(entity, #output_types::new(#output_vnames)) {
                                        log::error!("insert component failed {}", err);
                                    }
                                }
                            )*
                            #purge_done
                };
                let run_code = if self.dynamic {
                    quote! {
                       if let Some(symbol) = self.lib.get_symbol(&dm) {
                            #run_code
                       } else {
                            log::error!("symbol not found for system {}", #func_name);
                        }
                    }
                } else {
                    quote! {
                        #run_code
                    }
                };
                quote! {
                    impl<'a> ::specs::System<'a> for #system_name {
                        type SystemData = (
                            #(#system_data,)*
                        );

                        fn run(&mut self, (#(#input_names,)*): Self::SystemData) {
                           #run_code
                        }
                    }
                }
            }
            SystemType::Double => quote!(),
            SystemType::Multiple => quote!(),
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
    Resource,
    State,
    Input,
    Component,
}

enum Parameter {
    Component(Ident, usize, bool),
    Resource(Ident, usize, bool),
    State(Ident, usize, bool),
    Input(Ident),
    Entity(Ident),
}

struct Sig {
    ident: Ident,
    parameters: Vec<Parameter>,
    state_args: Vec<Type>,
    resource_args: Vec<Type>,
    component_args: Vec<Type>,
    input: Option<Type>,
    outputs: Vec<Type>,
    output_names: Vec<Ident>,
}

impl Sig {
    fn has_entity(&self) -> bool {
        self.parameters.iter().any(|param| match param {
            Parameter::Entity(_) => true,
            _ => false,
        })
    }

    fn parse(item: &mut Signature) -> Result<Self, Error> {
        let mut parameters = Vec::new();
        let mut resource_args = Vec::new();
        let mut state_args = Vec::new();
        let mut component_args = Vec::new();
        let mut input = None;
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
                                Some(ArgAttr::Resource) => {
                                    parameters.push(Parameter::Resource(
                                        name,
                                        resource_args.len(),
                                        mutable,
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
                                Some(ArgAttr::Input) => {
                                    parameters.push(Parameter::Input(name));
                                    if input.replace(elem.clone()).is_some() {
                                        return Err(Error::MultipleInputFound);
                                    }
                                }
                                _ => {
                                    if is_entity(elem) {
                                        if mutable {
                                            return Err(Error::EntityCantBeMutable(arg.span()));
                                        }
                                        let name = format_ident!("entity");
                                        parameters.push(Parameter::Entity(name));
                                    } else {
                                        parameters.push(Parameter::Component(
                                            name,
                                            component_args.len(),
                                            mutable,
                                        ));
                                    }
                                    component_args.push(elem.clone());
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
            input,
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

    fn is_return_type(&self, typ: &Type) -> bool {
        self.outputs.iter().any(|ty| typ == ty)
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

#[proc_macro_derive(ChangeSet)]
pub fn derive_changeset(item: TokenStream) -> TokenStream {
    let code = quote!();
    TokenStream::from(code)
}

#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemFn);
    input.vis = Visibility::Inherited;
    let name = input.sig.ident.clone();
    let pname = format_ident!("__private_{}", name);

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
                    Type::Reference(r) => {
                        if r.mutability.is_some() {
                            input_names.push(quote!(unsafe {::std::mem::transmute(#name)}));
                        } else {
                            input_names.push(quote!(#name));
                        }
                        r.mutability = None;
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
        fn #name(#(#call_names:#input_types,)*) -> ::std::thread::Result<#return_type> {
            ::std::panic::catch_unwind(||#pname(#(#input_names,)*))
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
