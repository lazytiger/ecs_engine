use crate::{generator::gen_io_config, Error};
use convert_case::{Case, Casing};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use std::path::PathBuf;

fn keep_order_dispatch(
    cmds: &Vec<u32>,
    files: &Vec<Ident>,
    names: &Vec<Ident>,
    vnames: &Vec<Ident>,
) -> TokenStream {
    quote!(
        fn dispatch(&mut self, ident:RequestIdent, data:Vec<u8>) {
            if let Err(err) = match ident {
                RequestIdent::Token(token) => self.token.send(token).map_err(|err|format!("{}", err)),
                RequestIdent::Close(entity) => {
                    if !self.input_cache.contains_key(&entity) {
                        self.input_cache.insert(entity, (true, VecDeque::new()));
                    }
                    let (next, cache) = self.input_cache.get_mut(&entity).unwrap();
                    if *next {
                        self.input_cache.remove(&entity);
                        self.close
                            .send((entity, Closing(true)))
                            .map_err(|err| format!("{}", err))
                    } else {
                        cache.push_back(AllRequest::Closing(Closing(true)));
                        Ok(())
                    }
                },
                RequestIdent::Entity(entity) => {
                    if !self.input_cache.contains_key(&entity) {
                        self.input_cache.insert(entity, (true, VecDeque::new()));
                    }
                    let (next, cache) = self.input_cache.get_mut(&entity).unwrap();

                    let mut buffer = data.as_slice();
                    let cmd = BigEndian::read_u32(buffer);
                    buffer = &buffer[4..];
                    match cmd {
                        #(
                            #cmds => {
                                let mut data = #files::#names::new();
                                data.merge_from_bytes(buffer).unwrap();
                                let data = #names::new(data);
                                if *next && cache.is_empty() {
                                    *next = false;
                                    self.#vnames.send((entity, data)).map_err(|err|format!("{}", err))
                                } else {
                                    if self.keep_duplicate {
                                        cache.push_back(AllRequest::#names(data));
                                    } else {
                                        if let Some(AllRequest::#names(old)) = cache.back_mut() {
                                            *old = data;
                                        } else {
                                            cache.push_back(AllRequest::#names(data));
                                        }
                                    }
                                    if *next {
                                        self.do_next(entity);
                                    }
                                    Ok(())
                                }
                            },
                        )*
                            _ => {
                                log::error!("invalid cmd:{}", cmd);
                                self.close
                                    .send((entity, Closing(false)))
                                    .map_err(|err| format!("{}", err))
                            },
                    }
                }
            } {
                    log::error!("send request to ecs failed:{}", err);
            }
        }
    )
}

fn disorder_dispatch(
    cmds: &Vec<u32>,
    files: &Vec<Ident>,
    names: &Vec<Ident>,
    vnames: &Vec<Ident>,
) -> TokenStream {
    quote!(
        fn dispatch(&mut self, ident:RequestIdent, data:Vec<u8>) {
            if let Err(err) = match ident {
                RequestIdent::Token(token) => self.token.send(token).map_err(|err|format!("{}", err)),
                RequestIdent::Close(entity) => self.close
                    .send((entity, Closing(true)))
                    .map_err(|err| format!("{}", err)),
                RequestIdent::Entity(entity) => {
                    let mut buffer = data.as_slice();
                    let cmd = BigEndian::read_u32(buffer);
                    buffer = &buffer[4..];
                    match cmd {
                        #(
                            #cmds => {
                                let mut data = #files::#names::new();
                                data.merge_from_bytes(buffer).unwrap();
                                let data = #names::new(data);
                                self.#vnames.send((entity, data)).map_err(|err|format!("{}", err))
                            },
                        )*
                            _ => {
                                log::error!("invalid cmd:{}", cmd);
                                self.close
                                    .send((entity, Closing(false)))
                                    .map_err(|err| format!("{}", err))
                            },
                    }
                }
            } {
                    log::error!("send request to ecs failed:{}", err);
            }
        }
    )
}

fn keep_order_do_next(names: &Vec<Ident>, vnames: &Vec<Ident>) -> TokenStream {
    quote!(
        fn do_next(&mut self, entity:Entity) {
            let mut clean = false;
            if let Some((next, cache)) = self.input_cache.get_mut(&entity) {
                if cache.is_empty() {
                    *next = true;
                } else {
                    if let Err(err) = {
                        match cache.pop_front().unwrap() {
                            #(AllRequest::#names(data) => self.#vnames.send((entity, data)).map_err(|err|format!("{}", err)),)*
                            AllRequest::Closing(data) => {
                                clean = true;
                                self.close.send((entity, data)).map_err(|err|format!("{}", err))
                            }
                        }
                    } {
                        log::error!("send request to ecs failed:{}", err);
                    }
                }
            }
            if clean {
                self.input_cache.remove(&entity);
            }
        }
    )
}

pub fn gen_request(
    keep_order: bool,
    keep_duplicate: bool,
    request_dir: PathBuf,
    config_dir: PathBuf,
    proto_dir: PathBuf,
) -> Result<(), Error> {
    gen_io_config(
        "request",
        request_dir,
        config_dir,
        proto_dir,
        |_configs, mods, names, files, inners, cmds| {
            let vnames: Vec<_> = names
                .iter()
                .map(|name| format_ident!("{}", name.to_string().to_case(Case::Snake)))
                .collect();
            let snames: Vec<_> = names
                .iter()
                .map(|name| format!("{}_input", name.to_string().to_case(Case::Snake)))
                .collect();
            let cnames: Vec<_> = names
                .iter()
                .map(|name| format!("{}_cleanup", name.to_string().to_case(Case::Snake)))
                .collect();
            let enames: Vec<_> = names
                .iter()
                .map(|name| format!("{}_exec", name.to_string().to_case(Case::Snake)))
                .collect();

            let cleanup = if keep_order {
                quote!(
                    pub fn cleanup(&self, builder:&mut GameDispatcherBuilder) {
                    #(
                        builder.add(CleanStorageSystem::<#names>::new(self.next_sender.clone()), #cnames, &[#enames]);
                    )*
                    }
                )
            } else {
                quote!(
                    pub fn cleanup(&self, builder:&mut GameDispatcherBuilder) {
                    #(
                        builder.add(CleanStorageSystem::<#names>::default(), #cnames, &[#enames]);
                    )*
                }
                )
            };

            let dispatch = if keep_order {
                keep_order_dispatch(&cmds, &files, &names, &vnames)
            } else {
                disorder_dispatch(&cmds, &files, &names, &vnames)
            };

            let all_request = if keep_order {
                quote!(
                    enum AllRequest {
                        #(#names(#names),)*
                        Closing(Closing),
                    }
                )
            } else {
                quote!(
                    enum AllRequest {}
                )
            };

            let do_next = if keep_order {
                keep_order_do_next(&names, &vnames)
            } else {
                quote!(
                    fn do_next(&mut self, entity: Entity) {}
                )
            };

            let code = quote!(
                    #![allow(dead_code)]
                    #![allow(unused_variables)]
                    #(mod #mods;)*

                    use byteorder::{BigEndian, ByteOrder};
                    use crossbeam::channel::{Receiver, Sender};
                    use ecs_engine::{
                        channel, CleanStorageSystem,  Closing, HandshakeSystem, HashComponent, Input,
                        InputSystem, RequestIdent, CommandId, GameDispatcherBuilder,
                    };
                    use mio::Token;
                    use protobuf::Message;
                    use specs::Entity;
                    use std::collections::{HashMap, VecDeque};

                    #(pub type #names = HashComponent<#files::#names>;)*
                    #(pub use #inners;)*

                    #all_request

                    pub struct Request {
                        keep_duplicate:bool,
                        input_cache: HashMap<Entity, (bool, VecDeque<AllRequest>)>,
                        next_receiver: Receiver<Vec<Entity>>,
                        next_sender: Sender<Vec<Entity>>,
                        token:Sender<Token>,
                        close:Sender<(Entity, Closing)>,
                        #(#vnames: Sender<(Entity, #names)>,)*
                    }

                    impl Request {
                        pub fn new(bounded_size: usize, builder: &mut GameDispatcherBuilder) -> Self {
                            let (next_sender, next_receiver) = channel(0);
                            let input_cache = HashMap::new();
                            let (token, receiver) = channel(bounded_size);
                            builder.add(HandshakeSystem::new(receiver), "handshake", &[]);
                            let (close, receiver) = channel(bounded_size);
                            builder.add(InputSystem::new(receiver), "close_input", &[]);
                            #(
                                let (#vnames, receiver) = channel(bounded_size);
                                builder.add(InputSystem::new(receiver), #snames, &[]);
                            )*
                            Self {
                                keep_duplicate:#keep_duplicate, token, close, next_receiver, next_sender, input_cache,
                                #(#vnames,)*
                            }
                        }

                        #cleanup

                    }

                    #(
                        impl CommandId<#names> for Request {
                            fn cmd(_:&#names) -> u32 {
                                #cmds
                            }
                        }
                    )*

                    impl Input for Request {

                        #dispatch

                        fn next_receiver(&self) -> Receiver<Vec<Entity>> {
                            self.next_receiver.clone()
                        }

                        #do_next

                    }
                ).to_string();
            Ok(code)
        },
    )
}
