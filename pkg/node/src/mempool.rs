use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::Arc;
use std::vec;
use tokio::sync::oneshot;

struct MempoolTxn<Txn, Change, ChanOkVal> {
    txn: Txn,
    sender: Option<oneshot::Sender<Result<ChanOkVal, crate::Error>>>,
    changes: Vec<Change>,
}

#[derive(Clone)]
pub struct Mempool<Key, Txn, Lease, Change, ChanOkVal> {
    #[allow(clippy::type_complexity)]
    state: Arc<Mutex<MempoolState<Key, Txn, Lease, Change, ChanOkVal>>>,
}

pub struct MempoolState<Key, Txn, Lease, Change, ChanOkVal> {
    txns: HashMap<Key, MempoolTxn<Txn, Change, ChanOkVal>>,
    pool: VecDeque<Key>,
    leased: HashMap<Lease, HashSet<Key>>,
}

// Manual default impls to avoid unnecessary trait bounds
impl<Key, Txn, Lease, Change, ChanOkVal> Default for Mempool<Key, Txn, Lease, Change, ChanOkVal> {
    fn default() -> Self {
        Self {
            state: Arc::default(),
        }
    }
}

impl<Key, Txn, Lease, Change, ChanOkVal> Default
    for MempoolState<Key, Txn, Lease, Change, ChanOkVal>
{
    fn default() -> Self {
        Self {
            txns: HashMap::default(),
            pool: VecDeque::default(),
            leased: HashMap::default(),
        }
    }
}

impl<K, V, L, C, CV> Mempool<K, V, L, C, CV>
where
    K: Eq + PartialEq + Hash + Clone + std::fmt::Debug,
    V: Clone + std::fmt::Debug,
    L: Eq + PartialEq + Hash + Clone + std::fmt::Debug,
    C: Eq + PartialEq + Hash + Clone,
{
    /// Add a transaction to the mempool, only adds key/txn if the key
    /// doesn't already exist in the mempool. This is used when other nodes
    /// send us a txn they have received from a client
    pub fn add(&self, key: K, txn: V, changes: Vec<C>) {
        self._add(key, txn, changes, None);
    }

    /// Add a transaction to the mempool and wait for it to be committed. This will only
    /// be called where the txn is directly submitted to this node from a client
    // TODO: handle duplicate add_wait, so we properly await
    pub async fn add_wait(&self, key: K, txn: V, changes: Vec<C>) -> Result<CV, crate::Error> {
        let (send, recv) = oneshot::channel::<Result<CV, crate::Error>>();
        self._add(key, txn, changes, Some(send));

        recv.await.expect("recv error")
    }

    /// Internal add function, used by both add and add_wait
    fn _add(
        &self,
        key: K,
        txn: V,
        changes: Vec<C>,
        sender: Option<oneshot::Sender<Result<CV, crate::Error>>>,
    ) {
        let mut state = self.state.lock();

        if state.txns.contains_key(&key) {
            return;
        }

        state.txns.entry(key.clone()).or_insert(MempoolTxn {
            txn,
            sender,
            changes,
        });

        // Add the key to the pool
        state.pool.push_back(key);
    }

    /// Commit a given transaction with key, removing it from the mempool
    /// and resolving any waiting futures (from add_txn_wait)
    #[allow(clippy::type_complexity)]
    pub fn commit(&self, lease: L, keys_with_results: Vec<(&K, Result<CV, crate::Error>)>) {
        let mut state = self.state.lock();

        for (key, result) in keys_with_results {
            if let Some(mem_txn) = state.txns.remove(key) {
                if let Some(sender) = mem_txn.sender {
                    let _ = sender.send(result);
                }
            }

            if let Some(lease) = state.leased.get_mut(&lease) {
                lease.remove(key);
            }

            if let Some(pos) = state.pool.iter().position(|x| x == key) {
                // TODO: this is very inefficient, we should find a better way to do this
                state.pool.remove(pos);
            }
        }

        // Drop lock before calling free with lock
        drop(state);

        // Free the leaseed items that are not committed
        self.free(lease);
    }

    /// Free a set of leased txns, these txns will now be unlocked and
    /// available for other leases
    fn free(&self, lease: L) {
        let mut state = self.state.lock();

        // Get the keys in the lease, and push them back into the pool, putting
        // them first so they are highest priority
        state
            .leased
            .remove(&lease)
            .unwrap_or_default()
            .into_iter()
            .for_each(|k| {
                state.pool.push_front(k);
            });
    }

    /// Lease a specific key (based on another commit)
    pub fn lease_txns(&self, lease: L, keys: &[K]) {
        let mut state = self.state.lock();

        for key in keys {
            // Remove from pool if exists
            let k = state
                .pool
                .iter()
                .position(|x| x == key)
                .and_then(|p| state.pool.remove(p));

            // Get an owned version of the key
            let key = if let Some(k) = k { k } else { key.clone() };

            // Add it to the lease
            state
                .leased
                .entry(lease.clone())
                .or_insert(HashSet::new())
                .insert(key);
        }
    }

    /// Lease a set of txns, these txns will now be locked until the lease
    /// is committed
    pub fn lease_batch(&self, lease: L, max_count: usize) -> Vec<(K, V)> {
        let mut state = self.state.lock();
        let mut txns = vec![];
        let mut discard = vec![];
        let mut conflict_check = HashSet::new();

        while let Some(key) = state.pool.pop_front() {
            #[allow(clippy::expect_used)]
            let changes = state
                .txns
                .get(&key)
                .expect("key not found in txns")
                .changes
                .clone();

            // A change key has already been included in a previously added txn
            if changes.iter().any(|c| conflict_check.contains(c)) {
                discard.push(key);
                continue;
            }

            state
                .leased
                .entry(lease.clone())
                .or_insert(HashSet::new())
                .insert(key.clone());

            conflict_check.extend(changes);

            #[allow(clippy::unwrap_used)]
            txns.push((key.clone(), state.txns.get(&key).unwrap().txn.clone()));

            // If we have reached the max count, break
            if txns.len() >= max_count {
                break;
            }
        }

        // Return the discarded keys to the pool
        state.pool.extend(discard);

        txns
    }
}

#[cfg(test)]
mod tests {
    use std::{thread::sleep, time::Duration};

    use super::*;
    use tokio::runtime::Runtime;

    type Mp = Mempool<String, u32, usize, usize, ()>;

    #[test]
    fn test_add_txn() {
        let mempool = Mp::default();
        mempool.add("key1".to_string(), 42, vec![]);

        {
            let state = mempool.state.lock();
            assert_eq!(state.txns.len(), 1);
            assert_eq!(state.txns.get("key1").unwrap().txn, 42);
        }

        mempool.add("key1".to_string(), 24, vec![]);

        {
            let state = mempool.state.lock();
            assert_eq!(state.txns.len(), 1);
            assert_eq!(state.pool.len(), 1);
            assert_eq!(state.txns.get("key1").unwrap().txn, 42);
        }

        {
            let state = mempool.state.lock();
            assert_eq!(state.txns.len(), 1);
            assert_eq!(state.txns.get("key1").unwrap().txn, 42);
        }
    }

    #[test]
    fn test_add_txn_wait() {
        let mempool = Arc::new(Mp::default());
        let rt = Runtime::new().unwrap();

        let mempool2 = mempool.clone();
        rt.spawn(async move {
            mempool2
                .add_wait("key1".to_string(), 42, vec![])
                .await
                .unwrap();
        });

        sleep(Duration::from_millis(100));

        {
            let state = mempool.state.lock();
            assert_eq!(state.txns.len(), 1);
            assert_eq!(state.txns.get("key1").unwrap().txn, 42);
        }
    }

    #[test]
    fn test_commit_txn() {
        let mempool = Mp::default();
        mempool.add("key1".into(), 42, vec![]);
        mempool.add("key2".into(), 24, vec![]);

        mempool.commit(1, vec![(&"key1".to_string(), Ok(()))]);

        let state = mempool.state.lock();
        assert_eq!(state.txns.len(), 1);
        assert_eq!(state.pool.len(), 1);
        assert!(state.txns.get("key1").is_none());
        assert_eq!(state.txns.get("key2").unwrap().txn, 24);
    }

    #[test]
    fn test_lease_batch() {
        let mempool = Mp::default();
        mempool.add("key1".to_string(), 42, vec![]);
        mempool.add("key2".to_string(), 24, vec![]);
        mempool.add("key3".to_string(), 15, vec![]);

        let batch = mempool.lease_batch(2, 2);
        assert_eq!(batch.len(), 2);

        {
            let state = mempool.state.lock();
            assert_eq!(state.pool.len(), 1);
        }

        mempool.commit(2, vec![(&"key1".to_string(), Ok(()))]);

        let batch = mempool.lease_batch(2, 2);
        assert_eq!(batch.len(), 2);

        // assert_eq!(batch[1], ("key2".to_string(), 24));
        // assert_eq!(batch[0], ("key1".to_string(), 42));
    }

    #[test]
    fn test_lease_with_duplicate_changes() {
        let mempool = Mp::default();
        mempool.add("key1".to_string(), 42, vec![1, 2, 3]);
        mempool.add("key2".to_string(), 24, vec![3, 4, 5]);
        mempool.add("key3".to_string(), 15, vec![6, 7, 8]);

        let batch = mempool.lease_batch(2, 3);
        assert_eq!(batch.len(), 2);

        {
            let state = mempool.state.lock();
            assert_eq!(state.pool.len(), 1);
        }
    }

    #[test]
    fn test_partial_commit_followed_by_lease() {
        let mempool = Mp::default();
        mempool.add("key1".to_string(), 1, vec![1]);
        mempool.add("key2".to_string(), 2, vec![2]);
        mempool.add("key3".to_string(), 3, vec![3]);

        let batch = mempool.lease_batch(2, 3);
        assert_eq!(batch.len(), 3);

        mempool.commit(2, vec![(&"key1".to_string(), Ok(()))]);

        // After commiting key1, transactions key2 and key3 can be leased again
        let batch = mempool.lease_batch(2, 3);
        assert_eq!(batch.len(), 2);
    }
}
