use convert_case::{Case, Casing};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse_macro_input;
use syn::{FnArg, ItemFn, Meta, NestedMeta, Type};

#[proc_macro_attribute]
pub fn system(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemFn);
    let name = item.sig.ident.to_string();
    let system_name = format_ident!("{}System", name.to_case(Case::UpperCamel));

    for attr in &item.attrs {
        match attr.path.get_ident() {
            Some(ident) if ident == "dynamic" => {
                if let Ok(meta) = attr.parse_meta() {
                    match meta {
                        Meta::List(list) => {
                            for meta in &list.nested {
                                match meta {
                                    NestedMeta::Meta(meta) => match meta {
                                        Meta::NameValue(nv) => {}
                                        Meta::Path(path) => {}
                                        _ => todo!("no name"),
                                    },
                                    _ => todo!("no list"),
                                }
                            }
                        }
                        _ => todo!("no "),
                    }
                }
            }
            _ => todo!(),
        }
    }
    item.attrs.clear();

    for param in &mut item.sig.inputs {
        match param {
            FnArg::Receiver(_) => {
                todo!()
            }
            FnArg::Typed(arg) => match arg.ty.as_ref() {
                Type::Reference(ty) => {
                    let mutable = ty.mutability.is_some();
                    for i in 0..arg.attrs.len() {
                        match arg.attrs[i].path.get_ident() {
                            Some(ident) if ident == "org" => {}
                            Some(ident) if ident == "member" => {}
                            Some(ident) if ident == "resource" => {}
                            Some(ident) if ident == "state" => {}
                            _ => continue,
                        }
                        arg.attrs.remove(i);
                        break;
                    }
                }
                _ => todo!(),
            },
        }
    }

    let lname = "native";
    let fname = "test";
    let code = quote! {

        #[derive(Default)]
        struct #system_name {
            lib: ecs_engine::DynamicSystem<fn(&UserInfo, &BagInfo)>,
        }

        impl #system_name {
            pub fn setup(mut self, world: &mut specs::World, builder: &mut specs::DispatcherBuilder, dm: &ecs_engine::DynamicManager) {
                world.register::<UserInfo>();
                world.register::<BagInfo>();
                self.lib.init(#lname.into(), #fname.into(), dm);
                builder.add(self, #name, &[]);
            }
        }

        impl<'a> specs::System<'a> for #system_name {
            type SystemData = (
                specs::ReadStorage<'a, UserInfo>,
                specs::ReadStorage<'a, BagInfo>,
                specs::Read<'a, ecs_engine::DynamicManager>,
            );

            fn run(&mut self, (user, bag, dm): Self::SystemData) {
                if let Some(symbol) = self.lib.get_symbol(&dm) {
                    for (user, bag) in (&user, &bag).join() {
                        if let Err(err) = std::panic::catch_unwind(||{
                            (*symbol)(user, bag);
                        }) {
                            log::error!("execute system {} failed with {:?}", #name, err);
                        }
                    }
                } else {
                    log::error!("symbol not found for system {}", #name);
                }
            }
        }
    };
    TokenStream::from(code)
}
