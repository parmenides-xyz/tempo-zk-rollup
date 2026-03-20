use std::{
    marker::PhantomData,
    ops::{Bound, RangeBounds},
};

use crate::list::StoreList;
use primitives::{block_height::BlockHeight, pagination::CursorChoice};
use rocksdb::DB;
use wire_message::WireMessage;

use crate::{list::List, Block, Error, Result};

pub(crate) trait StoreKey: Clone {
    fn to_key(&self) -> Key;
    fn from_key(key: Key) -> Option<Self>
    where
        Self: Sized;

    fn serialize_to(&self, to: &mut Vec<u8>);
    fn deserialize(bytes: &[u8]) -> Result<Self>;
}

pub(crate) trait StoreValue {
    fn serialize(&self) -> Result<Vec<u8>>;
    fn deserialize(bytes: &[u8]) -> Result<Self>
    where
        Self: Sized;
}

impl<T> StoreValue for T
where
    T: WireMessage,
{
    fn serialize(&self) -> Result<Vec<u8>> {
        Ok(self.to_bytes()?)
    }

    fn deserialize(bytes: &[u8]) -> Result<Self> {
        Ok(Self::from_bytes(bytes)?)
    }
}

pub(crate) trait KeyOrder: Copy {
    /// If this is the default order the keys are indexed in.
    fn is_indexed_order(&self) -> bool;

    fn reverse(&self) -> Self;
}

pub(crate) trait ListableKey<Value>: StoreKey
where
    Value: StoreValue,
{
    type Order: KeyOrder;

    fn min_value() -> Self;
    fn max_value() -> Self;

    fn list<'db>(
        db: &'db DB,
        range: impl RangeBounds<Self>,
        order: &Self::Order,
    ) -> List<'db, Value>
    where
        Self: Sized,
    {
        List {
            db,
            start_key: match range.start_bound() {
                std::ops::Bound::Included(x) => x.to_key(),
                std::ops::Bound::Excluded(x) => x.to_key(),
                std::ops::Bound::Unbounded => Self::min_value().to_key(),
            },
            end_key: match range.end_bound() {
                std::ops::Bound::Included(x) => x.to_key(),
                std::ops::Bound::Excluded(x) => x.to_key(),
                std::ops::Bound::Unbounded => Self::max_value().to_key(),
            },
            lower_exclusive: match range.start_bound() {
                std::ops::Bound::Included(_) => false,
                std::ops::Bound::Excluded(_) => true,
                std::ops::Bound::Unbounded => false,
            },
            upper_inclusive: match range.end_bound() {
                std::ops::Bound::Included(_) => true,
                std::ops::Bound::Excluded(_) => false,
                std::ops::Bound::Unbounded => true,
            },
            start_to_end: order.is_indexed_order(),
            _phantom: PhantomData,
        }
    }

    fn list_paginated(
        db: &DB,
        cursor: &Option<CursorChoice<Self>>,
        order: Self::Order,
        limit: usize,
    ) -> Result<Vec<(Key, Value)>>
    where
        Self: Sized,
    {
        let (start, end) = match cursor {
            None => (Bound::Unbounded, Bound::Unbounded),
            Some(CursorChoice::Before(before)) => (Bound::Unbounded, before.to_bound().cloned()),
            Some(CursorChoice::After(after)) => (after.to_bound().cloned(), Bound::Unbounded),
        };

        let (start, end) = match order.is_indexed_order() {
            true => (start, end),
            false => (end, start),
        };

        let (reverse_results, order) = match cursor {
            None | Some(CursorChoice::After(_)) => (false, order),
            // Without this, if entities 1, 2, 3 exist and indexed order is LowToHigh,
            // Before(3) would return entity 1, instead of entity 2.
            // The list call would look like this: `self.list(..3, LowToHigh)`,
            // so we need to reverse the order, for the first .next() to return 2 instead of 1.
            // TODO: will this also work with keys whose indexed order is HighToLow?
            // We don't have any right now.
            Some(CursorChoice::Before(_)) => (true, order.reverse()),
        };

        let mut results = Self::list(db, (start, end), &order)
            .into_iterator()
            .take(limit)
            .collect::<Result<Vec<(Key, Value)>>>()?;

        // If the cursor is Before, we need to reverse the results, otherwise in the example above
        // the first .next() would return 2, and the next 1, which does not match
        // the expected LowToHigh order.
        if reverse_results {
            results.reverse();
        }

        Ok(results)
    }
}

#[derive(Debug, Clone)]
pub enum Key {
    Block(KeyBlock),
    MaxHeight,
    BlockHashToHeight([u8; 32]),
    PendingBlock,
    TxnByHash([u8; 32]),
    StoreVersion,
    NonEmptyBlock(KeyNonEmptyBlock),
}

impl Key {
    fn kind(&self) -> u8 {
        match self {
            Self::Block(_) => 0,
            Self::MaxHeight => 1,
            Self::BlockHashToHeight(_) => 2,
            Self::PendingBlock => 3,
            Self::TxnByHash(_) => 4,
            Self::StoreVersion => 5,
            Self::NonEmptyBlock(_) => 6,
        }
    }

    pub(crate) fn serialize(&self) -> Vec<u8> {
        let mut out = vec![self.kind()];

        match self {
            Self::Block(block_number) => {
                block_number.serialize_to(&mut out);
            }
            Self::MaxHeight => {}
            Self::BlockHashToHeight(block_hash) => {
                out.extend_from_slice(block_hash);
            }
            Self::PendingBlock => {}
            Self::TxnByHash(txn_hash) => {
                out.extend_from_slice(txn_hash);
            }
            Self::StoreVersion => {}
            Self::NonEmptyBlock(block_number) => {
                block_number.serialize_to(&mut out);
            }
        }

        out
    }

    pub(crate) fn deserialize(bytes: &[u8]) -> Result<Self> {
        let Some((kind, bytes)) = bytes.split_first() else {
            return Err(Error::InvalidKey);
        };

        match *kind {
            0 => KeyBlock::deserialize(bytes).map(Self::Block),
            1 => Ok(Self::MaxHeight),
            2 => {
                let mut block_hash = [0u8; 32];
                block_hash.copy_from_slice(&bytes[0..32]);
                Ok(Self::BlockHashToHeight(block_hash))
            }
            3 => Ok(Self::PendingBlock),
            4 => {
                let mut txn_hash = [0u8; 32];
                txn_hash.copy_from_slice(&bytes[0..32]);
                Ok(Self::TxnByHash(txn_hash))
            }
            5 => Ok(Self::StoreVersion),
            6 => KeyNonEmptyBlock::deserialize(bytes).map(Self::NonEmptyBlock),
            _ => Err(Error::InvalidKey),
        }
    }

    pub(crate) fn serialize_immediate_successor(&self) -> Vec<u8> {
        let mut out = self.serialize();
        out.push(0);
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyBlock(pub(crate) BlockHeight);

impl StoreKey for KeyBlock {
    fn to_key(&self) -> Key {
        Key::Block(self.clone())
    }

    fn from_key(key: Key) -> Option<Self> {
        match key {
            Key::Block(kb) => Some(kb),
            _ => None,
        }
    }

    fn serialize_to(&self, to: &mut Vec<u8>) {
        to.extend_from_slice(&self.0.to_be_bytes());
    }

    fn deserialize(bytes: &[u8]) -> Result<Self> {
        let Ok(u64_bytes) = TryInto::<[u8; 8]>::try_into(&bytes[0..8]) else {
            return Err(Error::InvalidKey);
        };

        let block_height = BlockHeight::from(u64::from_be_bytes(u64_bytes));
        Ok(KeyBlock(block_height))
    }
}

impl<V: StoreValue> ListableKey<V> for KeyBlock {
    type Order = BlockListOrder;

    fn min_value() -> Self {
        KeyBlock(BlockHeight(0))
    }

    fn max_value() -> Self {
        KeyBlock(BlockHeight(u64::MAX))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BlockListOrder {
    LowestToHighest,
    HighestToLowest,
}

impl KeyOrder for BlockListOrder {
    fn is_indexed_order(&self) -> bool {
        match self {
            Self::LowestToHighest => true,
            Self::HighestToLowest => false,
        }
    }

    fn reverse(&self) -> Self {
        match self {
            Self::LowestToHighest => Self::HighestToLowest,
            Self::HighestToLowest => Self::LowestToHighest,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyNonEmptyBlock(pub(crate) BlockHeight);

impl KeyNonEmptyBlock {
    pub(crate) fn from_block<B: Block>(block: &B) -> Option<Self> {
        if block.txns().is_empty() {
            None
        } else {
            Some(Self(block.block_height()))
        }
    }
}

impl StoreKey for KeyNonEmptyBlock {
    fn to_key(&self) -> Key {
        Key::NonEmptyBlock(KeyNonEmptyBlock(self.0))
    }

    fn from_key(key: Key) -> Option<Self> {
        match key {
            Key::NonEmptyBlock(key) => Some(key),
            _ => None,
        }
    }

    fn serialize_to(&self, to: &mut Vec<u8>) {
        to.extend_from_slice(&self.0.to_be_bytes());
    }

    fn deserialize(bytes: &[u8]) -> Result<Self> {
        let Ok(u64_bytes) = TryInto::<[u8; 8]>::try_into(&bytes[0..8]) else {
            return Err(Error::InvalidKey);
        };

        Ok(KeyNonEmptyBlock(BlockHeight(u64::from_be_bytes(u64_bytes))))
    }
}

impl<V: StoreValue> ListableKey<V> for KeyNonEmptyBlock {
    type Order = BlockListOrder;

    fn min_value() -> Self {
        KeyNonEmptyBlock(BlockHeight(0))
    }

    fn max_value() -> Self {
        KeyNonEmptyBlock(BlockHeight(u64::MAX))
    }
}
