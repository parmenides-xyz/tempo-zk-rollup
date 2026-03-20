// These features have been stabilized in recent Rust versions
#![allow(incomplete_features)]
#![feature(return_position_impl_trait_in_trait)]
#![feature(associated_type_defaults)]
#![feature(bound_map)]

mod keys;
mod list;
mod migration;

use std::{marker::PhantomData, path::Path};

use keys::{Key, KeyBlock, StoreKey};
use migration::LATEST_VERSION;
use primitives::block_height::BlockHeight;
use rocksdb::DB;
use wire_message::WireMessage;

pub use keys::BlockListOrder;
pub use list::StoreList;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid key")]
    InvalidKey,

    #[error("invalid version '{0}'")]
    InvalidVersion(u32),

    #[error("rocksdb error: {0}")]
    RocksDB(#[from] rocksdb::Error),

    #[error("wire message error: {0}")]
    WireMessage(#[from] wire_message::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T, E = Error> = std::result::Result<T, E>;

pub struct BlockStore<B> {
    db: DB,
    _marker: PhantomData<B>,
}

pub trait Block {
    type Txn: Transaction;

    fn block_height(&self) -> BlockHeight;
    fn block_hash(&self) -> [u8; 32];

    fn txns(&self) -> Vec<Self::Txn>;
}

pub trait Transaction {
    fn txn_hash(&self) -> [u8; 32];
}

impl<B> BlockStore<B>
where
    B: Block + WireMessage,
    B::Txn: WireMessage,
{
    fn db_options(create_if_missing: bool) -> rocksdb::Options {
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(create_if_missing);
        opts
    }

    pub fn create_or_load(path: &Path) -> Result<Self> {
        if path.exists() && std::fs::read_dir(path)?.next().is_some() {
            Self::load_existing(path)
        } else {
            Self::create(path)
        }
    }

    fn create(path: &Path) -> Result<Self> {
        let db = DB::open(&Self::db_options(true), path)?;

        let self_ = Self {
            db,
            _marker: PhantomData,
        };

        self_.set_store_version(LATEST_VERSION)?;

        Ok(self_)
    }

    fn load_existing(path: &Path) -> Result<Self> {
        let db = DB::open(&Self::db_options(false), path)?;

        Ok(Self {
            db,
            _marker: PhantomData,
        })
    }

    pub fn set(&self, block: &B) -> Result<()> {
        // Use a batch to write atomically in case we crash in the middle
        let mut batch = rocksdb::WriteBatchWithTransaction::<false>::default();

        let height = block.block_height();
        let block_hash = block.block_hash();

        batch.put(Key::Block(KeyBlock(height)).serialize(), block.to_bytes()?);

        let max_height = self.get_max_height()?;
        if max_height.map_or(true, |max_height| height > max_height) {
            batch.put(Key::MaxHeight.serialize(), height.to_be_bytes());
        }

        batch.put(
            Key::BlockHashToHeight(block_hash).serialize(),
            height.to_be_bytes(),
        );

        for e in Self::txn_entries(block) {
            let (k, v) = e?;

            batch.put(k.serialize(), v);
        }

        if let Some(key) = keys::KeyNonEmptyBlock::from_block(block) {
            batch.put(key.to_key().serialize(), block.to_bytes()?);
        }

        self.db.write(batch)?;

        Ok(())
    }

    fn txn_entries(block: &B) -> impl Iterator<Item = Result<(Key, Vec<u8>)>> + '_ {
        block
            .txns()
            .into_iter()
            .map(move |tx| Ok((Key::TxnByHash(tx.txn_hash()), tx.to_bytes()?)))
    }

    pub fn get(&self, block_number: BlockHeight) -> Result<Option<B>> {
        let key = Key::Block(KeyBlock(block_number)).serialize();

        let block_bytes = self.db.get(key)?;
        let block = block_bytes.map(|bytes| B::from_bytes(&bytes)).transpose()?;

        Ok(block)
    }

    pub fn get_max_height(&self) -> Result<Option<BlockHeight>> {
        if let Some(max_block) = self.db.get(Key::MaxHeight.serialize())? {
            Ok(Some(BlockHeight(u64::from_be_bytes(
                max_block.try_into().unwrap(),
            ))))
        } else {
            Ok(None)
        }
    }

    pub fn get_block_height_by_hash(&self, block_hash: [u8; 32]) -> Result<Option<BlockHeight>> {
        let key_bytes = Key::BlockHashToHeight(block_hash).serialize();

        if let Some(block_height) = self.db.get(key_bytes)? {
            Ok(Some(BlockHeight(u64::from_be_bytes(
                block_height.try_into().unwrap(),
            ))))
        } else {
            Ok(None)
        }
    }

    pub fn get_pending_block(&self) -> Result<Option<B>> {
        let key = Key::PendingBlock;
        let bytes = self.db.get(key.serialize())?;
        let block = bytes.map(|bytes| B::from_bytes(&bytes)).transpose()?;

        Ok(block)
    }

    pub fn get_txn_by_hash(&self, txn_hash: [u8; 32]) -> Result<Option<B::Txn>> {
        let key = Key::TxnByHash(txn_hash);
        let bytes = self.db.get(key.serialize())?;

        if let Some(bytes) = bytes {
            Ok(Some(B::Txn::from_bytes(&bytes)?))
        } else {
            Ok(None)
        }
    }

    fn store_version(&self) -> Result<u32> {
        if let Some(version) = self.db.get(Key::StoreVersion.serialize())? {
            Ok(u32::from_be_bytes(version.try_into().unwrap()))
        } else {
            Ok(0)
        }
    }

    fn set_store_version(&self, version: u32) -> Result<()> {
        self.db
            .put(Key::StoreVersion.serialize(), version.to_be_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempdir::TempDir;

    pub(crate) type DummyBlock =
        wire_message::test_api::DummyMsg<(BlockHeight, [u8; 32], Vec<DummyTxn>)>;
    pub(crate) type DummyTxn = wire_message::test_api::DummyMsg<[u8; 32]>;

    impl Block for DummyBlock {
        type Txn = DummyTxn;

        fn block_height(&self) -> BlockHeight {
            self.inner().0
        }

        fn block_hash(&self) -> [u8; 32] {
            self.inner().1
        }

        fn txns(&self) -> Vec<Self::Txn> {
            self.inner().2.clone()
        }
    }

    impl Transaction for DummyTxn {
        fn txn_hash(&self) -> [u8; 32] {
            *self.inner()
        }
    }

    fn temp_dir() -> TempDir {
        TempDir::new("block-store").unwrap()
    }

    #[test]
    fn test_create_or_load() {
        let temp_dir = temp_dir();
        BlockStore::<DummyBlock>::create_or_load(temp_dir.path()).unwrap();
    }

    #[test]
    fn test_set_and_get() {
        let temp_dir = temp_dir();
        let block_store = BlockStore::<DummyBlock>::create_or_load(temp_dir.path()).unwrap();

        let block_number = BlockHeight(1);
        let txns = vec![DummyTxn::V1([123; 32]), DummyTxn::V1([124; 32])];
        let block_data = DummyBlock::V1((block_number, [0; 32], txns.clone()));

        block_store.set(&block_data).unwrap();
        let retrieved_data = block_store.get(block_number).unwrap().unwrap();
        assert_eq!(&retrieved_data, &block_data);

        let max_height = block_store.get_max_height().unwrap().unwrap();
        assert_eq!(max_height, block_number);

        let block_height_by_hash = block_store
            .get_block_height_by_hash([0u8; 32])
            .unwrap()
            .unwrap();

        assert_eq!(block_height_by_hash, block_number);

        assert_eq!(
            block_store
                .get_txn_by_hash(txns[0].txn_hash())
                .unwrap()
                .unwrap(),
            txns[0]
        );

        assert_eq!(
            block_store
                .get_txn_by_hash(txns[1].txn_hash())
                .unwrap()
                .unwrap(),
            txns[1]
        );

        assert_eq!(block_store.get_txn_by_hash([125; 32]).unwrap(), None);

        let listed_txns = block_store.list_txns().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(listed_txns, txns);
    }

    #[test]
    fn test_list_blocks() {
        let temp_dir = temp_dir();
        let block_store = BlockStore::<DummyBlock>::create_or_load(temp_dir.path()).unwrap();

        // Insert some blocks
        for i in 0..10_000 {
            block_store
                .set(&DummyBlock::V1((BlockHeight(i as u64), [0; 32], vec![])))
                .unwrap();
        }

        let blocks: Vec<_> = block_store
            .list(
                BlockHeight(3)..BlockHeight(8),
                BlockListOrder::LowestToHighest,
            )
            .into_iterator()
            .collect::<Result<_>>()
            .unwrap();

        assert_eq!(blocks.len(), 5);
        for (i, (block_number, data)) in blocks.into_iter().enumerate() {
            assert_eq!(block_number, KeyBlock(BlockHeight((i + 3) as u64)));
            assert_eq!(
                &data,
                &DummyBlock::V1((BlockHeight(i as u64 + 3), [0; 32], vec![]))
            );
        }

        // Paginated list
        let blocks = block_store
            .list_paginated(&None, BlockListOrder::LowestToHighest, usize::MAX)
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        // Same order, but backwards
        let before_blocks = block_store
            .list_paginated(
                &Some(primitives::pagination::CursorChoice::Before(
                    primitives::pagination::CursorChoiceBefore::BeforeInclusive(
                        blocks.last().unwrap().1.block_height(),
                    ),
                )),
                BlockListOrder::LowestToHighest,
                blocks.len(),
            )
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        // The order when iterating backwards
        // should still be consistent with the requested LowestToHighest order,
        // such that the lowest height blocks are first.
        assert_eq!(blocks, before_blocks);

        // Same thing, but with a limit that's less than the max number of blocks
        let before_blocks_except_first = block_store
            .list_paginated(
                &Some(primitives::pagination::CursorChoice::Before(
                    primitives::pagination::CursorChoiceBefore::BeforeInclusive(
                        blocks.last().unwrap().1.block_height(),
                    ),
                )),
                BlockListOrder::LowestToHighest,
                blocks.len() - 1,
            )
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(blocks[1..], before_blocks_except_first);
    }

    #[test]
    fn successor() {
        let temp_dir = temp_dir();
        let db = BlockStore::<DummyBlock>::create_or_load(temp_dir.path()).unwrap();

        let height = BlockHeight(u64::MAX);
        db.set(&DummyBlock::V1((height, [0; 32], vec![]))).unwrap();
        db.db
            .put(
                Key::Block(KeyBlock(BlockHeight(u64::MAX))).serialize_immediate_successor(),
                b"test",
            )
            .unwrap();

        assert_eq!(
            db.list(.., BlockListOrder::LowestToHighest)
                .into_iterator()
                .count(),
            1
        );
    }
}
