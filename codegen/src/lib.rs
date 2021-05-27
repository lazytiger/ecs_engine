use proc_macro::TokenStream;
use syn::parse_macro_input;
use quote::quote;
use syn::{ItemFn, FnArg, Type, Meta, NestedMeta};

#[proc_macro_attribute]
pub fn system(attr:TokenStream, item:TokenStream) ->TokenStream {
    let mut item = parse_macro_input!(item as ItemFn);
    for attr in &item.attrs {
        match attr.path.get_ident() {
            Some(ident) if ident == "dynamic" => {
                if let Ok(meta) = attr.parse_meta() {
                    match meta {
                        Meta::List(list) => {
                            for meta in &list.nested {
                                match meta {
                                    NestedMeta::Meta(meta) => {
                                        match meta {
                                            Meta::NameValue(nv) => {
                                            },
                                            Meta::Path(path) => {

                                            }
                                            _ => todo!("no name"),
                                        }
                                    }
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
            FnArg::Typed(arg) => {
                match arg.ty.as_ref() {
                    Type::Reference(ty) => {
                        let mutable = ty.mutability.is_some();
                        for i in 0..arg.attrs.len() {
                            match arg.attrs[i].path.get_ident() {
                                Some(ident) if ident == "org"  => {
                                }
                                Some(ident) if ident == "member" => {
                                }
                                Some(ident) if ident == "resource" => {
                                }
                                Some(ident) if ident == "state" => {

                                }
                                _ => {continue},
                            }
                            arg.attrs.remove(i);
                            break;
                        }
                    }
                    _ => todo!()
                }
            }
        }
    }
    TokenStream::from(quote!(#item))
}
