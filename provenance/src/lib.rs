//! Pure maths and types for the screened pool (E10, Privacy Pools on LEZ): the lots a private
//! account holds, the ASP's approved set and its Merkle tree, and the window of roots a withdrawal
//! may prove against.
//!
//! No `lee_core` types (constraints §9), like `bosphorus-core`, but a crate of its own: the lending
//! guest builds from `bosphorus-core`, and any change there changes its image id, so every deployed
//! lending instance would stop matching the client. The hash is passed in ([`Hasher`]), so the
//! guest uses the zkVM's SHA-256 accelerator and this crate stays free of risc0.

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    ZeroAmount,
    UnknownLot,
    InsufficientLot,
    LotNotAsClaimed,
    TooManyLots,
    DuplicateLabel,
    UnknownLabel,
    TooManyLabels,
    BadPath,
    NotApproved,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Self::ZeroAmount => "zero amount",
            Self::UnknownLot => "no lot of this asset with this label",
            Self::InsufficientLot => "the lot holds less than that",
            Self::LotNotAsClaimed => "the lot doesn't hold the claimed amount",
            Self::TooManyLots => "too many lots on one account",
            Self::DuplicateLabel => "label already present",
            Self::UnknownLabel => "label not in the approved set",
            Self::TooManyLabels => "approved set is full",
            Self::BadPath => "malformed Merkle path",
            Self::NotApproved => "label not under the claimed root",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

pub type Hash = [u8; 32];
/// A deposit's identity, unique per deposit (`bosphorus_pool::deposit_label`).
pub type Label = Hash;
/// A token instance's header id, or `0x00…00` for the native token.
pub type Asset = [u8; 32];
pub type Hasher = fn(&[u8]) -> Hash;

/// Lots per account: one per deposit, so this bounds the deposits one account holds at once.
pub const MAX_LOTS: usize = 32;
/// Lots one withdrawal may spend.
pub const MAX_SPENDS: usize = 8;
/// Depth of the ASP's tree: every path has this many siblings.
pub const DEPTH: usize = 11;
/// The approved set lives on chain (64 KiB of a 100 KiB shard), so wallets need no ASP server.
pub const MAX_LABELS: usize = 1 << DEPTH;
/// Labels one approval adds, and removals one tx makes: each costs a scan of the set and a path of
/// hashes, and the ASP's tx must stay under the wallet's gas limit (E10 has the numbers).
pub const MAX_APPROVE: usize = 32;
pub const MAX_REMOVE: usize = 8;
/// Roots a withdrawal may use. Approvals only add labels, so an older root is a subset of the
/// current one; a removal clears the older roots.
pub const MAX_ROOTS: usize = 8;
/// An empty subtree at any height, and a removed leaf. No leaf or node hashes to it.
pub const EMPTY: Hash = [0; 32];
/// A removed label's place in [`Approved::leaves`] (positions never move, so paths stay valid).
pub const REMOVED: Label = [0; 32];

/// What one deposit left on its holder's account: `amount` of `asset`, still carrying the
/// deposit's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Lot {
    pub asset: Asset,
    pub label: Label,
    pub amount: u128,
}

/// `shard[pool]` of a holder.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Lots(pub Vec<Lot>);

impl Lots {
    /// A deposit. Labels are unique (each deposit's record is written once).
    pub fn credit(&mut self, lot: Lot) -> Result<()> {
        if lot.amount == 0 {
            return Err(Error::ZeroAmount);
        }
        if self.0.iter().any(|l| l.label == lot.label) {
            return Err(Error::DuplicateLabel);
        }
        if self.0.len() >= MAX_LOTS {
            return Err(Error::TooManyLots);
        }
        self.0.push(lot);
        Ok(())
    }

    /// A withdrawal of `amount` from one lot; an emptied lot goes away.
    pub fn debit(&mut self, asset: &Asset, label: &Label, amount: u128) -> Result<()> {
        if amount == 0 {
            return Err(Error::ZeroAmount);
        }
        let i = self.find(asset, label)?;
        let lot = &mut self.0[i];
        lot.amount = lot.amount.checked_sub(amount).ok_or(Error::InsufficientLot)?;
        if lot.amount == 0 {
            self.0.remove(i);
        }
        Ok(())
    }

    /// A ragequit: the whole lot, which must hold exactly `amount`.
    pub fn take(&mut self, asset: &Asset, label: &Label, amount: u128) -> Result<()> {
        let i = self.find(asset, label)?;
        if self.0[i].amount != amount {
            return Err(Error::LotNotAsClaimed);
        }
        self.0.remove(i);
        Ok(())
    }

    pub fn total(&self, asset: &Asset) -> u128 {
        self.0.iter().filter(|l| l.asset == *asset).fold(0, |sum, l| sum.saturating_add(l.amount))
    }

    fn find(&self, asset: &Asset, label: &Label) -> Result<usize> {
        self.0.iter().position(|l| l.asset == *asset && l.label == *label).ok_or(Error::UnknownLot)
    }
}

/// The ASP's approved labels as an incremental Merkle tree of depth [`DEPTH`] (as Tornado's and
/// Privacy Pools' trees): `filled` keeps the last left node of each level, so adding a label costs
/// one path of hashes, not a pass over the set. A removal blanks the label's leaf and recomputes its
/// path, which the ASP supplies and the current root checks. The root is always derived here, so
/// the published list and the root can't disagree.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Approved {
    /// In tree order; [`REMOVED`] where a label was removed.
    pub leaves: Vec<Label>,
    pub filled: Vec<Hash>,
    pub root: Hash,
}

impl Default for Approved {
    fn default() -> Self {
        Self { leaves: vec![], filled: vec![EMPTY; DEPTH], root: EMPTY }
    }
}

/// One label out of the approved set, with its path in the tree as it stands.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Removal {
    pub label: Label,
    pub path: MerklePath,
}

impl Approved {
    pub fn contains(&self, label: &Label) -> bool {
        *label != REMOVED && self.leaves.contains(label)
    }

    pub fn index_of(&self, label: &Label) -> Option<usize> {
        if *label == REMOVED { None } else { self.leaves.iter().position(|l| l == label) }
    }

    /// The labels in the set, without the removed ones.
    pub fn labels(&self) -> impl Iterator<Item = &Label> {
        self.leaves.iter().filter(|l| **l != REMOVED)
    }

    pub fn approve(&mut self, h: Hasher, labels: &[Label]) -> Result<()> {
        if labels.len() > MAX_APPROVE {
            return Err(Error::TooManyLabels);
        }
        for label in labels {
            if *label == REMOVED || self.leaves.contains(label) {
                return Err(Error::DuplicateLabel);
            }
            if self.leaves.len() >= MAX_LABELS {
                return Err(Error::TooManyLabels);
            }
            let mut index = self.leaves.len();
            let mut acc = leaf(h, label);
            for filled in &mut self.filled {
                if index & 1 == 0 {
                    *filled = acc;
                    acc = node(h, &acc, &EMPTY);
                } else {
                    acc = node(h, filled, &acc);
                }
                index >>= 1;
            }
            self.root = acc;
            self.leaves.push(*label);
        }
        Ok(())
    }

    /// Removes one label, given its path in the tree as it stands (checked against the root).
    pub fn remove(&mut self, h: Hasher, removal: &Removal) -> Result<()> {
        let index = usize::try_from(removal.path.index).map_err(|_| Error::BadPath)?;
        if self.leaves.get(index) != Some(&removal.label) || removal.label == REMOVED {
            return Err(Error::UnknownLabel);
        }
        verify(h, &removal.label, &removal.path, &self.root)?;
        let next = self.leaves.len();
        let (mut i, mut acc) = (index, EMPTY);
        for (level, sibling) in removal.path.siblings.iter().enumerate() {
            // The next approval reads `filled[level]` as its left sibling when its own position
            // there is odd: if that node is on this path, it changes too.
            let up = next >> level;
            if up & 1 == 1 && i == up - 1 {
                self.filled[level] = acc;
            }
            acc = if i & 1 == 0 { node(h, &acc, sibling) } else { node(h, sibling, &acc) };
            i >>= 1;
        }
        self.root = acc;
        self.leaves[index] = REMOVED;
        Ok(())
    }

    /// `leaves[index]`'s path, from the whole tree (for clients; the program never needs it).
    pub fn path(&self, h: Hasher, index: usize) -> Option<MerklePath> {
        if index >= self.leaves.len() {
            return None;
        }
        let mut level: Vec<Hash> = self.leaves.iter().map(|l| leaf_value(h, l)).collect();
        let (mut i, mut siblings) = (index, vec![]);
        for _ in 0..DEPTH {
            siblings.push(*level.get(i ^ 1).unwrap_or(&EMPTY));
            level = up(h, &level);
            i >>= 1;
        }
        Some(MerklePath { index: u32::try_from(index).ok()?, siblings })
    }
}

/// Accepted roots, oldest first.
pub fn push_root(roots: &mut Vec<Hash>, root: Hash, flush: bool) {
    if flush {
        roots.clear();
    }
    roots.push(root);
    if roots.len() > MAX_ROOTS {
        roots.remove(0);
    }
}

/// From a leaf up: `index`'s bits say at each level whether the node is a right child.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MerklePath {
    pub index: u32,
    pub siblings: Vec<Hash>,
}

/// `H(0x00 ‖ label)`. Leaves and nodes hash under different prefixes, so a node can't pass as a leaf.
pub fn leaf(h: Hasher, label: &Label) -> Hash {
    let mut bytes = [0u8; 33];
    bytes[1..].copy_from_slice(label);
    h(&bytes)
}

fn leaf_value(h: Hasher, label: &Label) -> Hash {
    if *label == REMOVED { EMPTY } else { leaf(h, label) }
}

fn node(h: Hasher, left: &Hash, right: &Hash) -> Hash {
    let mut bytes = [0u8; 65];
    bytes[0] = 1;
    bytes[1..33].copy_from_slice(left);
    bytes[33..].copy_from_slice(right);
    h(&bytes)
}

/// One level up; a missing right child is [`EMPTY`].
fn up(h: Hasher, level: &[Hash]) -> Vec<Hash> {
    level.chunks(2).map(|pair| node(h, &pair[0], pair.get(1).unwrap_or(&EMPTY))).collect()
}

/// The root of `leaves` computed in full: what [`Approved`] keeps incrementally.
pub fn root(h: Hasher, leaves: &[Label]) -> Hash {
    if leaves.is_empty() {
        return EMPTY;
    }
    let mut level: Vec<Hash> = leaves.iter().map(|l| leaf_value(h, l)).collect();
    for _ in 0..DEPTH {
        level = up(h, &level);
    }
    level[0]
}

/// `label` is a leaf of the tree with this root.
pub fn verify(h: Hasher, label: &Label, path: &MerklePath, root: &Hash) -> Result<()> {
    if path.siblings.len() != DEPTH || u64::from(path.index) >> DEPTH != 0 {
        return Err(Error::BadPath);
    }
    let mut acc = leaf(h, label);
    for (level, sibling) in path.siblings.iter().enumerate() {
        acc = if path.index >> level & 1 == 0 { node(h, &acc, sibling) } else { node(h, sibling, &acc) };
    }
    if acc == *root { Ok(()) } else { Err(Error::NotApproved) }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn h(bytes: &[u8]) -> Hash {
        use sha2::Digest as _;
        sha2::Sha256::digest(bytes).into()
    }

    fn label(i: u16) -> Label {
        let mut l = [0xAB; 32];
        l[..2].copy_from_slice(&i.to_le_bytes());
        l
    }

    proptest! {
        /// Whatever the ASP adds and removes, the kept root is the full tree's, every label in the
        /// set proves under it, and no removed or outside label does.
        #[test]
        fn the_incremental_tree_is_the_full_tree(ops in prop::collection::vec((any::<bool>(), 0u16..40), 1..60)) {
            let mut set = Approved::default();
            let mut next = 0u16;
            for (add, pick) in ops {
                let live: Vec<usize> = (0..set.leaves.len()).filter(|i| set.leaves[*i] != REMOVED).collect();
                if add || live.is_empty() {
                    set.approve(h, &[label(next)]).unwrap();
                    next += 1;
                } else {
                    let index = live[usize::from(pick) % live.len()];
                    let removal = Removal { label: set.leaves[index], path: set.path(h, index).unwrap() };
                    set.remove(h, &removal).unwrap();
                }
                prop_assert_eq!(set.root, root(h, &set.leaves));
            }
            for (i, l) in set.leaves.iter().enumerate() {
                let p = set.path(h, i).unwrap();
                prop_assert_eq!(verify(h, l, &p, &set.root).is_ok(), *l != REMOVED);
                prop_assert!(verify(h, &label(9999), &p, &set.root).is_err());
            }
        }
    }

    #[test]
    fn a_removal_needs_the_labels_true_path() {
        let mut set = Approved::default();
        set.approve(h, &[label(1), label(2), label(3)]).unwrap();
        assert_eq!(set.approve(h, &[label(2)]), Err(Error::DuplicateLabel));
        let wrong = Removal { label: label(2), path: set.path(h, 0).unwrap() };
        assert!(set.remove(h, &wrong).is_err(), "another leaf's path");
        let mut forged = set.path(h, 1).unwrap();
        forged.siblings[3] = [7; 32];
        assert!(set.remove(h, &Removal { label: label(2), path: forged }).is_err(), "a forged sibling");
        set.remove(h, &Removal { label: label(2), path: set.path(h, 1).unwrap() }).unwrap();
        assert!(set.remove(h, &Removal { label: label(2), path: set.path(h, 1).unwrap() }).is_err(), "twice");
    }

    #[test]
    fn lots_never_pay_out_more_than_was_deposited() {
        let (asset, label) = ([1; 32], [2; 32]);
        let mut lots = Lots::default();
        lots.credit(Lot { asset, label, amount: 100 }).unwrap();
        assert_eq!(lots.credit(Lot { asset, label, amount: 1 }), Err(Error::DuplicateLabel));
        assert_eq!(lots.debit(&[9; 32], &label, 1), Err(Error::UnknownLot), "another asset");
        assert_eq!(lots.debit(&asset, &label, 101), Err(Error::InsufficientLot));
        lots.debit(&asset, &label, 60).unwrap();
        assert_eq!(lots.take(&asset, &label, 41), Err(Error::LotNotAsClaimed));
        lots.take(&asset, &label, 40).unwrap();
        assert_eq!(lots, Lots::default());
    }
}
