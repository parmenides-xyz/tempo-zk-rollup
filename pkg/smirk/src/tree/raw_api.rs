use crate::{hash_cache::HashCache, Collision, Element, Tree};

use super::error::StructName;

impl<const DEPTH: usize, V, C> Tree<DEPTH, V, C>
where
    C: HashCache,
{
    /// Insert into the tree and btreemap at the same time, without updating the hash
    pub(crate) fn insert_without_hashing(
        &mut self,
        entries: Vec<(Element, V)>,
    ) -> Result<(), Collision> {
        if entries.is_empty() {
            return Ok(());
        }

        if entries.iter().any(|(e, _)| e == &Element::NULL_HASH) {
            return Err(Collision {
                inserted: Element::NULL_HASH,
                in_tree: Element::NULL_HASH,
                depth: DEPTH,
                struct_name: StructName::Tree,
            });
        }

        if let Some((element, _)) = entries.iter().find(|(e, _)| self.entries.contains_key(e)) {
            return Err(Collision {
                in_tree: *element,
                inserted: *element,
                depth: DEPTH,
                struct_name: StructName::Tree,
            });
        }

        let mut elements_and_bits = Vec::with_capacity(entries.len());
        for (element, _) in &entries {
            // if the tree has depth n, we need n-1 bits, since there are n-1 left/right decisions
            elements_and_bits.push((element, element.lsb(DEPTH - 1).to_bitvec()));
        }
        elements_and_bits.sort_unstable_by(|(_, a_bits), (_, b_bits)| a_bits.cmp(b_bits));

        let (elements, bits): (Vec<_>, Vec<_>) = elements_and_bits.into_iter().unzip();

        let result = self
            .tree
            .insert_without_hashing::<DEPTH>(&elements, &bits, 0)?;

        match result {
            true => {
                for (element, value) in entries {
                    self.entries.insert(element, value);
                }
            }
            false => unreachable!(
                "we check if the tree contains the element earlier, so this should be impossible"
            ),
        };

        Ok(())
    }
}
