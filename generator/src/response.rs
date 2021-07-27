use crate::{generator::gen_io_config, Error, Trait};
use quote::{format_ident, quote};
use std::path::PathBuf;

pub fn gen_response(
    response_dir: PathBuf,
    config_dir: PathBuf,
    proto_dir: PathBuf,
) -> Result<(), Error> {
    gen_io_config(
        "response",
        response_dir,
        config_dir,
        proto_dir,
        |configs, mods, names, files, inners, cmds| {
            let mut drop_entity = quote!();
            for (_, cf) in configs {
                for config in cf.configs {
                    let name = format_ident!("{}", config.name);
                    if let Some(traits) = config.traits {
                        for t in traits {
                            if let Trait::DropEntity { entities } = t {
                                if !drop_entity.is_empty() {
                                    return Err(Error::DuplicateDropEntity);
                                }
                                let fname =
                                    format_ident!("{}", entities.unwrap_or("mut_entities".into()));
                                drop_entity = quote!(
                                    impl ecs_engine::DropEntity for #name {
                                        fn mut_entities(&mut self) -> &mut Vec<u32> {
                                            self.data.#fname()
                                        }
                                    }
                                );
                            }
                        }
                    }
                }
            }
            let code = quote!(
                #(mod #mods;)*

                use ecs_engine::Output;
                use protobuf::Message;
                use std::ops::{Deref, DerefMut};

                #(pub type #names = Response<#files::#names>;)*
                #(pub use #inners;)*

                #[derive(Default)]
                pub struct Response<T:Default> {
                    data:T
                }

                impl<T:Default> Deref for Response<T> {
                    type Target = T;

                    fn deref(&self) -> &Self::Target {
                        &self.data
                    }
                }

                impl<T:Default> DerefMut for Response<T> {
                    fn deref_mut(&mut self) -> &mut Self::Target {
                        &mut self.data
                    }
                }

                impl<T:Default> From<T> for Response<T> {
                    fn from(data: T) -> Self {
                        Response { data }
                    }
                }

                impl<T: Message + Default> Response<T> {
                    pub fn new() -> Self {
                        Self { data: T::new() }
                    }
                }

                #(
                    impl Output for #names {
                        fn cmd() -> u32 {
                            #cmds
                        }
                    }
                )*

                #drop_entity
            )
            .to_string();
            Ok(code)
        },
    )
}
