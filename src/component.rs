#![allow(dead_code)]
use crate::{backend::Output, BytesSender, SyncDirection};
use mio::Token;
use specs::{
    BitSet, Component, DenseVecStorage, Entity, FlaggedStorage, HashMapStorage, Join, ReadStorage,
    VecStorage,
};
use specs_hierarchy::Parent;
use std::{
    cmp::Ordering,
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

            pub fn into(self) -> T {
                self.data
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

    pub fn tokens<'a>(storage: &'a ReadStorage<'a, NetToken>, set: &BitSet) -> Vec<Token> {
        (storage, set)
            .join()
            .map(|(token, _)| token.token())
            .collect()
    }
}

#[derive(Default, Debug)]
pub struct Closing(pub bool);

impl Component for Closing {
    type Storage = HashMapStorage<Self>;
}

/// 单用于发送数据给自己
pub struct SelfSender {
    id: u32,
    token: Token,
    sender: BytesSender,
}

impl Component for SelfSender {
    type Storage = VecStorage<Self>;
}

impl SelfSender {
    pub fn new(id: u32, token: Token, sender: BytesSender) -> Self {
        Self { id, token, sender }
    }

    pub fn send_data(&self, data: impl Output) {
        self.sender.send_data(self.token, self.id, data);
    }

    pub fn send_close(&self, confirm: bool) {
        self.sender.send_close(self.token, confirm);
    }
}

pub struct Member<const T: usize> {
    entity: Entity,
}

impl<const T: usize> Member<T> {
    pub fn new(entity: Entity) -> Self {
        Self { entity }
    }
}

impl<const T: usize> Component for Member<T> {
    type Storage = FlaggedStorage<Self, VecStorage<Self>>;
}

impl<const T: usize> Parent for Member<T> {
    fn parent_entity(&self) -> Entity {
        self.entity
    }
}
/// 玩家的位置信息
pub trait Position {
    /// x轴坐标
    fn x(&self) -> f32;
    /// y轴坐标
    fn y(&self) -> f32;
}

/// 场景尺寸信息
pub trait SceneData: Clone {
    /// 场景id
    fn id(&self) -> u32;
    /// 场景坐标的最小xy值
    fn get_min_x(&self) -> f32;
    fn get_min_y(&self) -> f32;
    /// 获取场景的分块尺寸，即可以分为行列数
    fn get_column(&self) -> i32;
    fn get_row(&self) -> i32;
    /// 场景分隔的正方形边长
    fn grid_size(&self) -> f32;
    /// 根据位置信息计算格子索引
    /// index = y * column + x
    fn grid_index(&self, x: f32, y: f32) -> Option<usize> {
        let (min_x, min_y) = (self.get_min_x(), self.get_min_y());
        if x < min_x || y < min_y {
            return None;
        }
        let x = ((x - min_x) * 100.0) as i32;
        let y = ((y - min_y) * 100.0) as i32;
        let grid_size = (self.grid_size() * 100.0) as i32;
        let x = x / grid_size;
        let y = y / grid_size;
        let (row, column) = (self.get_row(), self.get_column());
        if x >= column || y >= row {
            return None;
        }
        Some((y * column + x) as usize)
    }
    /// 获取周围格子的索引，包括当前格子
    fn around(&self, index: usize) -> Vec<usize> {
        let mut data = Vec::new();
        let index = index as i32;
        let (row, column) = (self.get_row(), self.get_column());
        let x = index % column;
        let min_x = if x == 0 {
            0
        } else if x == column - 1 {
            column - 3
        } else {
            x - 1
        };
        let y = index / column;
        let min_y = if y == 0 {
            0
        } else if y == row - 1 {
            row - 3
        } else {
            y - 1
        };
        for x in min_x..min_x + 2 {
            for y in min_y..min_y + 2 {
                data.push((y * column + x) as usize)
            }
        }
        data
    }
    /// 根据旧的索引以及新索引来得到三个数据，分别代表删除，未变，新增
    fn diff(&self, old: usize, new: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let old = self.around(old);
        let new = self.around(new);
        let mut only_old = Vec::new();
        let mut only_new = Vec::new();
        let mut share = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < old.len() && j < new.len() {
            match old[i].cmp(&new[j]) {
                Ordering::Less => {
                    only_old.push(old[i]);
                    i += 1;
                }
                Ordering::Equal => {
                    share.push(old[i]);
                    i += 1;
                    j += 1;
                }
                Ordering::Greater => {
                    only_new.push(new[j]);
                    j += 1;
                }
            }
        }
        if i < old.len() {
            only_old.extend_from_slice(&old.as_slice()[i..]);
        }
        if j < new.len() {
            only_new.extend_from_slice(&new.as_slice()[j..]);
        }

        (only_old, share, only_new)
    }
}
pub type TeamMember = Member<0>;
pub type SceneMember = Member<1>;

#[derive(Default)]
pub struct FullDataCommit<const T: usize> {
    mask: BitSet,
}

impl<const T: usize> FullDataCommit<T> {
    pub fn dir() -> SyncDirection {
        T.into()
    }
    pub fn mask(&self) -> &BitSet {
        &self.mask
    }

    pub fn add(&mut self, id: u32) {
        self.mask.add(id);
    }

    pub fn add_mask(&mut self, mask: &BitSet) {
        self.mask |= mask;
    }
}

impl<const T: usize> Component for FullDataCommit<T> {
    type Storage = HashMapStorage<Self>;
}

pub type AroundFullData = FullDataCommit<1>;
pub type TeamFullData = FullDataCommit<8>;
