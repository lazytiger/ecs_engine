#![allow(dead_code)]
use crate::{Output, ResponseSender};
use mio::Token;
use specs::{Component, DenseVecStorage, HashMapStorage, NullStorage, VecStorage};
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

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
pub struct SelfSender<T> {
    token: Token,
    sender: ResponseSender,
    _phantom: PhantomData<T>,
}

impl<T> Component for SelfSender<T>
where
    T: Sync + Send + 'static,
{
    type Storage = VecStorage<Self>;
}

impl<T> SelfSender<T>
where
    T: Output,
{
    pub fn new(token: Token, sender: ResponseSender) -> Self {
        Self {
            token,
            sender,
            _phantom: Default::default(),
        }
    }

    pub fn send_data(&self, data: impl Into<T>) {
        let data = data.into().encode();
        self.sender.send_data(self.token, data);
    }

    pub fn send_close(&self, confirm: bool) {
        self.sender.send_close(self.token, confirm);
    }
}
