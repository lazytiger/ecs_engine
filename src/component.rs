#![allow(dead_code)]
use crate::{Output, ResponseSender};
use mio::Token;
use specs::{Component, DenseVecStorage, HashMapStorage, NullStorage, VecStorage};
use std::ops::{Deref, DerefMut};

macro_rules! component {
    ($storage:ident, $name:ident) => {
        #[derive(Debug, Default)]
        pub struct $name<T: Default> {
            data: T,
        }

        impl<T: Default> Component for $name<T>
        where
            T: 'static + Sync + Send,
        {
            type Storage = $storage<Self>;
        }
        impl<T: Default> Deref for $name<T> {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                &self.data
            }
        }

        impl<T: Default> DerefMut for $name<T> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.data
            }
        }

        impl<T: Default> $name<T> {
            pub fn new(data: T) -> Self {
                Self { data }
            }
        }
    };
}

component!(HashMapStorage, HashComponent);
component!(VecStorage, VecComponent);
component!(DenseVecStorage, DenseVecComponent);

pub type NetToken = VecComponent<usize>;

impl NetToken {
    pub fn token(&self) -> Token {
        Token(self.data)
    }
}

#[derive(Default)]
pub struct Closing;

impl Component for Closing {
    type Storage = NullStorage<Self>;
}

/// 单用于发送数据给自己
pub struct SelfSender {
    token: Token,
    sender: ResponseSender,
}

impl Component for SelfSender {
    type Storage = VecStorage<Self>;
}

impl SelfSender {
    pub fn new(token: Token, sender: ResponseSender) -> Self {
        Self { token, sender }
    }

    pub fn send_data<O: Output>(&self, data: O) {
        let data = data.encode();
        self.sender.send_data(self.token, data);
    }

    pub fn send_close(&self, confirm: bool) {
        self.sender.send_close(self.token, confirm);
    }
}
