use bitvec::{prelude::Msb0, vec::BitVec};

use crate::{hash::empty_tree_hash, hash_cache::HashCache, Collision, Element};

use super::StructName;

/// A tree-like representation of a sparse tree, for easier computation of merkle paths and hashes
#[derive(Debug, Clone)]
pub(crate) enum Node {
    /// A single leaf at the max depth of the tree
    Leaf(Element),

    /// A tree of depth `depth` containing only null elements
    ///
    /// Since these trees are well-known, all hashes can be computed ahead of time and refered to
    /// by lookup table
    Empty { depth: usize },

    /// A parent of two nodes with a cached hash
    Parent {
        left: Box<Self>,
        right: Box<Self>,
        hash: Element,
        /// if true, the children have changed without recalculating the hash
        hash_dirty: bool,
    },
}

impl Node {
    pub fn hash_with<const DEPTH: usize, C: HashCache>(
        &self,
        cache: &C,
        extra_elements: Vec<Element>,
    ) -> Element {
        let mut extra_elements_with_bits = extra_elements
            .into_iter()
            .map(|e| (e, e.lsb(DEPTH - 1).to_bitvec()))
            .collect::<Vec<_>>();

        extra_elements_with_bits.sort_unstable_by(|(_, a_bits), (_, b_bits)| a_bits.cmp(b_bits));

        let (elements, bits): (Vec<_>, Vec<_>) = extra_elements_with_bits.into_iter().unzip();
        self.hash_with_inner::<DEPTH, C>(cache, &elements, &bits, 0)
    }

    fn hash_with_inner<const DEPTH: usize, C: HashCache>(
        &self,
        cache: &C,
        extra_elements: &[Element],
        extra_elements_bits: &[BitVec<u8, Msb0>],
        path_depth: usize,
    ) -> Element {
        match self {
            Self::Leaf(element) => *element,
            Self::Parent { left, right, .. } => {
                let right_start = extra_elements_bits
                    .iter()
                    .position(|b| b[path_depth])
                    .unwrap_or(extra_elements_bits.len());
                let lefts = &extra_elements_bits[..right_start];
                let lefts_elements = &extra_elements[..right_start];
                let rights = &extra_elements_bits[right_start..];
                let rights_elements = &extra_elements[right_start..];

                let left_hash = match lefts.is_empty() {
                    true => left.hash(),
                    false => left.hash_with_inner::<DEPTH, C>(
                        cache,
                        lefts_elements,
                        lefts,
                        path_depth + 1,
                    ),
                };
                let right_hash = match rights.is_empty() {
                    true => right.hash(),
                    false => right.hash_with_inner::<DEPTH, C>(
                        cache,
                        rights_elements,
                        rights,
                        path_depth + 1,
                    ),
                };

                cache.hash(left_hash, right_hash)
            }
            Self::Empty { depth: 1 } => {
                // we need to check whether there should be an element here
                assert!(extra_elements.len() <= 1, "too many elements");
                extra_elements
                    .first()
                    .copied()
                    .unwrap_or_else(|| empty_tree_hash(1))
            }
            Self::Empty { depth } => {
                let child = Self::Parent {
                    left: Box::new(Self::Empty { depth: *depth - 1 }),
                    right: Box::new(Self::Empty { depth: *depth - 1 }),
                    hash: empty_tree_hash(*depth),
                    hash_dirty: false,
                };

                child.hash_with_inner::<DEPTH, C>(
                    cache,
                    extra_elements,
                    extra_elements_bits,
                    path_depth,
                )
            }
        }
    }

    pub fn hash(&self) -> Element {
        match self {
            Self::Leaf(hash) | Self::Parent { hash, .. } => *hash,
            Self::Empty { depth } => empty_tree_hash(*depth),
        }
    }

    /// Insert an element and return whether the value changed
    ///
    /// This does not update hashes, instead it marks nodes as "dirty" meaning the hash is
    /// potentially out of date
    ///
    /// The elements and bits should be sorted by the bits before calling this function
    pub(crate) fn insert_without_hashing<const N: usize>(
        &mut self,
        elements: &[Element],
        bits: &[BitVec<u8, Msb0>],
        path_depth: usize,
    ) -> Result<bool, Collision> {
        match self {
            Self::Leaf(e) if elements.contains(e) => Ok(false),
            Self::Leaf(e)
                if bits.iter().any({
                    let e_lsb = e.lsb(N - 1);
                    move |b| b == &e_lsb[..]
                }) =>
            {
                Err(Collision {
                    in_tree: *e,
                    inserted: *elements
                        .iter()
                        .find(|e| e.lsb(N - 1) == e.lsb(N - 1))
                        .unwrap(),
                    depth: N,
                    struct_name: StructName::Tree,
                })
            }
            Self::Leaf(_) => unreachable!(),
            // Self::Leaf(e) => {
            //
            //     dbg!(&e, &element, e.lsb(N - 1), element.lsb(N - 1));
            //     *e = element;
            //     Ok(true)
            // }
            Self::Parent {
                left,
                right,
                hash_dirty,
                ..
            } => {
                let rights_start = bits
                    .iter()
                    .position(|b| b[path_depth])
                    .unwrap_or(bits.len());
                let lefts = &bits[..rights_start];
                let rights = &bits[rights_start..];
                let lefts_elements = &elements[..rights_start];
                let rights_elements = &elements[rights_start..];

                let (left, right) = match (lefts.is_empty(), rights.is_empty()) {
                    (true, true) => return Ok(false),
                    (false, true) => (
                        { left.insert_without_hashing::<N>(lefts_elements, lefts, path_depth + 1) },
                        Ok(false),
                    ),
                    (true, false) => (Ok(false), {
                        right.insert_without_hashing::<N>(rights_elements, rights, path_depth + 1)
                    }),
                    (false, false) => (
                        right.insert_without_hashing::<N>(rights_elements, rights, path_depth + 1),
                        left.insert_without_hashing::<N>(lefts_elements, lefts, path_depth + 1),
                    ),
                };

                *hash_dirty = matches!(right, Ok(true)) || matches!(left, Ok(true));

                right?;
                left?;

                Ok(*hash_dirty)
            }
            Self::Empty { depth: 1 } => {
                assert_eq!(elements.len(), 1);
                *self = Self::Leaf(elements.first().copied().unwrap());
                Ok(true)
            }

            Self::Empty { depth } => {
                // split an empty tree into two empty subtrees
                *self = Self::Parent {
                    left: Box::new(Self::Empty { depth: *depth - 1 }),
                    right: Box::new(Self::Empty { depth: *depth - 1 }),
                    hash: empty_tree_hash(*depth),
                    hash_dirty: false,
                };

                // now try again
                self.insert_without_hashing::<N>(elements, bits, path_depth)
            }
        }
    }

    pub fn recalculate_hashes<C: HashCache>(
        &mut self,
        cache: &C,
        hash_remove_callback: &(impl Fn((&Element, &Element)) + Send + Sync),
        hash_set_callback: &(impl Fn((&Element, &Element, &Element)) + Send + Sync),
    ) {
        let Self::Parent {
            left,
            right,
            hash,
            hash_dirty,
        } = self
        else {
            return;
        };

        if !*hash_dirty {
            return;
        }

        let left_hash_before = left.hash();
        let right_hash_before = right.hash();
        hash_remove_callback((&left_hash_before, &right_hash_before));

        rayon::join(
            || left.recalculate_hashes(cache, hash_remove_callback, hash_set_callback),
            || right.recalculate_hashes(cache, hash_remove_callback, hash_set_callback),
        );

        let left_hash = left.hash();
        let right_hash = right.hash();
        *hash = cache.hash(left_hash, right_hash);
        *hash_dirty = false;
        hash_set_callback((&left_hash, &right_hash, hash));
    }
}

#[cfg(test)]
mod tests {
    use proptest::prop_assume;
    use test_strategy::proptest;

    use crate::{Batch, Tree};

    #[proptest]
    fn root_hash_with_matches_insert(mut tree: Tree<16, i32>, batch: Batch<16, i32>) {
        let hash_with = tree.root_hash_with(&batch.elements().collect::<Vec<_>>());
        let result = tree.insert_batch(batch, |_| {}, |_| {});

        prop_assume!(result.is_ok());

        assert_eq!(tree.root_hash(), hash_with);
    }
}
