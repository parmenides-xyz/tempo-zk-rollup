use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use smirk::Element;

use crate::{aggregate_utxo::AggregateUtxo, Snark, UTXO_INPUTS, UTXO_OUTPUTS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSet {
    Six,
    Eight,
    Nine,
    Fourteen,
    TwentyOne,
}

#[derive(Clone, Debug)]
pub struct Burn<const L: usize> {
    pub secret_key: Element,
    pub notes: [Note; L],
    pub to_address: Element,
}

// https://github.com/rust-lang/rust/issues/61415
impl<const L: usize> Default for Burn<L> {
    fn default() -> Self {
        Self {
            secret_key: Element::default(),
            notes: core::array::from_fn(|_| Note::default()),
            to_address: Element::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BurnTo<const L: usize> {
    pub secret_key: Element,
    pub notes: [Note; L],
    pub kind: Element,
    pub to_address: Element,
}

// https://github.com/rust-lang/rust/issues/61415
impl<const L: usize> Default for BurnTo<L> {
    fn default() -> Self {
        Self {
            secret_key: Element::default(),
            notes: core::array::from_fn(|_| Note::default()),
            to_address: Element::default(),
            kind: Element::default(),
        }
    }
}

// TODO: change Fr to Element
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Note {
    /// Address of owner of the note (AKA nullifer key or nk, a commitment to the secret key)
    pub address: Element,
    /// Blake2 hash with salts for increased entropy
    pub psi: Element,
    /// Value of the note
    pub value: Element,
    /// Kind of note
    pub token: String,
    /// Source of note (should be ethereum address)
    pub source: Element,
}

#[derive(Clone, Debug)]
pub struct Mint<const L: usize> {
    pub notes: [Note; L],
}

// https://github.com/rust-lang/rust/issues/61415
impl<const L: usize> Default for Mint<L> {
    fn default() -> Self {
        Self {
            notes: [(); L].map(|_| Note::default()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Insert<const MERKLE_D: usize> {
    /// Leaf node
    pub leaf: Element,
    /// Sibling path (does not include leaf or root)
    pub path: MerklePath<MERKLE_D>,
}

/// The siblings of a merkle path, for a [`smirk::Tree`] of depth `DEPTH`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerklePath<const DEPTH: usize> {
    /// The siblings that form the merkle path
    pub siblings: Vec<Element>,
}

impl<const DEPTH: usize> Default for MerklePath<DEPTH> {
    fn default() -> Self {
        let siblings = (1..DEPTH).map(smirk::empty_tree_hash).collect::<Vec<_>>();

        assert_eq!(siblings.len(), DEPTH - 1);

        Self { siblings }
    }
}

#[derive(Clone, Debug)]
pub struct Batch<const INSERTS: usize, const MERKLE_D: usize> {
    /// Inserts must link to each other, in other words the new root of the first element must match
    /// the old root of the second element, and so on.
    pub inserts: [Insert<MERKLE_D>; INSERTS],
}

impl<const INSERTS: usize, const MERKLE_D: usize> Default for Batch<INSERTS, MERKLE_D> {
    fn default() -> Self {
        Self {
            inserts: core::array::from_fn(|_| Insert::default()),
        }
    }
}

/// InputNote is a Note that belongs to the current user, i.e. they have the
/// spending sercret key and can therefore use it as an input, "spending" the note. Extra
/// constraints need to be applied to input notes to ensure they are valid.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InputNote<const MERKLE_D: usize> {
    pub note: Note,
    /// Secret key for the address, required to spend a note
    pub secret_key: Element,
    /// Input notes merkle tree path, so we can verify that the note exists
    /// in the tree, without revealing which hash it is
    /// Path for tree that matches recent root
    pub merkle_path: MerklePath<MERKLE_D>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Utxo<const MERKLE_D: usize> {
    pub inputs: [InputNote<MERKLE_D>; UTXO_INPUTS],
    pub outputs: [Note; UTXO_OUTPUTS],

    /// Merkle root of the input notes (required to prove that input notes already
    /// exist in the tree and can therefore be spent)
    pub root: Element,

    // Kind of transaction
    pub kind: UtxoKind,
}

impl<const MERKLE_D: usize> Default for Utxo<MERKLE_D> {
    fn default() -> Self {
        Self {
            inputs: core::array::from_fn(|_| InputNote::default()),
            outputs: core::array::from_fn(|_| Note::default()),
            root: Element::ZERO,
            kind: UtxoKind::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UtxoKind {
    Null,
    #[default]
    Transfer,
    Mint,
    Burn,
}

#[derive(
    Debug,
    Default,
    Clone,
    Hash,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct UTXOProof<const MERKLE_D: usize> {
    /// Root hash
    pub recent_root: Element,
    /// Mint/Burn hash (null for transfer)
    pub mb_hash: Element,
    /// Mint/Burn value (null for transfer)
    pub mb_value: Element,
    /// Leaves
    pub input_leaves: [Element; UTXO_INPUTS],
    pub output_leaves: [Element; UTXO_OUTPUTS],
    /// Proof
    pub proof: Vec<u8>,
}

/// The serialized form of a proof
#[derive(Debug, Clone, Serialize, Deserialize)]
#[wire_message::wire_message]
pub enum SnarkWitness {
    V1(SnarkWitnessV1),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SnarkWitnessV1 {
    pub instances: Vec<Vec<Element>>,
    #[serde(
        serialize_with = "crate::util::serialize_base64",
        deserialize_with = "crate::util::deserialize_base64"
    )]
    pub proof: Vec<u8>,
}

impl wire_message::WireMessage for SnarkWitness {
    type Ctx = ();
    type Err = core::convert::Infallible;

    fn version(&self) -> u64 {
        match self {
            Self::V1(_) => 1,
        }
    }

    fn upgrade_once(self, _ctx: &mut Self::Ctx) -> Result<Self, wire_message::Error> {
        Err(Self::max_version_error())
    }
}

#[derive(Clone, Default, Debug)]
pub struct Signature {
    /// Secret key for the address, required to spend a note
    pub secret_key: Element,
    /// Message to be signed
    pub message: Element,
}

#[derive(Clone, Debug)]
pub struct Points {
    /// Secret key
    pub secret_key: Element,
    /// Message to be signed
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug)]
pub struct AggregateAgg<const AGG_N: usize> {
    /// UTXO to aggregate
    pub aggregates: [Snark; AGG_N],

    /// Instances used to verify the proof
    pub agg_instances: Vec<Element>,

    /// Private witness to proof
    pub proof: Vec<u8>,
}

impl<const AGG_N: usize> Default for AggregateAgg<AGG_N> {
    fn default() -> Self {
        let aggregate_utxo = AggregateUtxo::<3, 161, 12>::default()
            .snark(ParameterSet::TwentyOne)
            .unwrap();

        Self::new(core::array::from_fn(|_| aggregate_utxo.clone()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NoteURLPayload {
    pub version: u8,
    pub private_key: Element,
    pub psi: Option<Element>,
    pub value: Element,
    pub referral_code: String,
}

pub fn decode_activity_url_payload(payload: &str) -> NoteURLPayload {
    let payload_bytes = bs58::decode(payload)
        .into_vec()
        .expect("Failed to decode base58 payload");

    let mut rest = &payload_bytes[..];

    let version = rest[0];
    rest = &rest[1..];

    let private_key_bytes: [u8; 32] = rest[..32]
        .try_into()
        .expect("Not enough bytes for private_key");
    let private_key = Element::from_be_bytes(private_key_bytes);
    rest = &rest[32..];

    let psi = if version == 0 {
        let psi_bytes: [u8; 32] = rest[..32].try_into().expect("Not enough bytes for psi");
        rest = &rest[32..];
        Some(Element::from_be_bytes(psi_bytes))
    } else {
        None
    };

    let leading_zeros = rest[0] as usize;
    rest = &rest[1..];

    let value_len = 32 - leading_zeros;
    let value_without_leading_zeros = &rest[..value_len];
    rest = &rest[value_len..];

    let mut value_bytes = [0u8; 32];
    value_bytes[leading_zeros..].copy_from_slice(value_without_leading_zeros);
    let value = Element::from_be_bytes(value_bytes);

    let referral_code = String::from_utf8(rest.to_vec()).expect("Invalid UTF-8 in referral code");

    NoteURLPayload {
        version,
        private_key,
        psi,
        value,
        referral_code,
    }
}

pub fn encode_activity_url_payload(payload: &NoteURLPayload) -> String {
    let mut bytes = Vec::new();

    // Encode version
    bytes.push(payload.version);

    // Encode private_key
    bytes.extend_from_slice(&payload.private_key.to_be_bytes());

    // Encode psi if version is 0
    if let Some(psi) = &payload.psi {
        if payload.version == 0 {
            bytes.extend_from_slice(&psi.to_be_bytes());
        }
    }

    // Encode value with leading zeros
    let value_bytes = payload.value.to_be_bytes();
    let leading_zeros = value_bytes.iter().take_while(|&&b| b == 0).count();
    bytes.push(leading_zeros as u8);
    bytes.extend_from_slice(&value_bytes[leading_zeros..]);

    // Encode referral_code as UTF-8
    bytes.extend_from_slice(payload.referral_code.as_bytes());

    // Return Base58-encoded string
    bs58::encode(bytes).into_string()
}
