use super::routes;
use crate::errors;
use primitives::hash::CryptoHash;
use rpc::{code::ErrorCode, error::HTTPError};
use serde::Serialize;
use zk_primitives::Element;

impl From<routes::error::Error> for HTTPError {
    fn from(err: routes::error::Error) -> Self {
        match err {
            routes::error::Error::InvalidElement(..) => HTTPError::new(
                ErrorCode::BadRequest,
                "invalid-element",
                Some(err.into()),
                None::<()>,
            ),
            routes::error::Error::OutOfSync => HTTPError::new(
                ErrorCode::Unavailable,
                "out-of-sync",
                Some(err.into()),
                None::<()>,
            ),
            routes::error::Error::StatisticsNotReady => HTTPError::new(
                ErrorCode::Unavailable,
                "statistics-not-ready",
                Some(err.into()),
                None::<()>,
            ),
            routes::error::Error::InvalidListQuery(err) => HTTPError::new(
                ErrorCode::BadRequest,
                "invalid-list-query",
                Some(err.into()),
                None::<()>,
            ),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ElementData {
    pub element: Element,
}

#[derive(Debug, Serialize)]
pub struct HashData {
    pub hash: CryptoHash,
}

#[derive(Debug, Serialize)]
pub struct ElementStringData {
    pub element: String,
}

impl From<errors::Error> for HTTPError {
    fn from(err: errors::Error) -> Self {
        match err {
            errors::Error::InvalidProof => HTTPError::new(
                ErrorCode::BadRequest,
                "invalid-proof",
                Some(err.into()),
                None::<()>,
            ),
            errors::Error::UtxoRootIsNotRecentEnough {
                utxo_recent_root, ..
            } => {
                // let allowed_recent_roots = err..clone();
                HTTPError::new(
                    ErrorCode::BadRequest,
                    "utxo-root-not-recent-enough",
                    Some(err.into()),
                    Some(ElementData {
                        element: utxo_recent_root,
                    }),
                )
            }
            errors::Error::ElementNotInTree { element } => HTTPError::new(
                ErrorCode::NotFound,
                "element-not-found",
                Some(err.into()),
                Some(ElementData { element }),
            ),
            errors::Error::NoteAlreadySpent {
                spent_note: nullifier,
                ..
            } => HTTPError::new(
                ErrorCode::AlreadyExists,
                "nullifier-conflict",
                Some(err.into()),
                Some(ElementData { element: nullifier }),
            ),
            errors::Error::OutputNoteExists {
                output_note: commitment,
            } => HTTPError::new(
                ErrorCode::AlreadyExists,
                "commitment-conflict",
                Some(err.into()),
                Some(ElementData {
                    element: commitment,
                }),
            ),
            errors::Error::MintIsNotInTheContract { key } => HTTPError::new(
                ErrorCode::NotFound,
                "mint-not-in-contract",
                Some(err.into()),
                Some(ElementData { element: key }),
            ),
            errors::Error::BurnIsNotInTheContract { key } => HTTPError::new(
                ErrorCode::NotFound,
                "burn-not-in-contract",
                Some(err.into()),
                Some(ElementData { element: key }),
            ),
            errors::Error::BurnToAddressCannotBeZero => HTTPError::new(
                ErrorCode::BadRequest,
                "burn-to-address-cannot-be-zero",
                Some(err.into()),
                None::<()>,
            ),
            errors::Error::InvalidElementSize { element } => HTTPError::new(
                ErrorCode::BadRequest,
                "invalid-element-modulus",
                Some(err.into()),
                Some(ElementData { element }),
            ),
            errors::Error::TxnNotFound { txn } => HTTPError::new(
                ErrorCode::NotFound,
                "txn-not-found",
                Some(err.into()),
                Some(HashData { hash: txn }),
            ),
            errors::Error::FailedToParseElement { element, source } => HTTPError::new(
                ErrorCode::BadRequest,
                "failed-to-parse-element",
                Some(source.into()),
                Some(ElementStringData { element }),
            ),
            _ => HTTPError::new(
                ErrorCode::Internal,
                "internal",
                Some(err.into()),
                None::<()>,
            ),
        }
    }
}
