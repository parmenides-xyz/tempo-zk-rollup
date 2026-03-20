use super::{error, State};
use actix_web::web;
use block_store::Block;
use primitives::hash::CryptoHash;
use rpc::error::HttpResult;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use zk_primitives::Element;

#[derive(Serialize)]
pub struct ElementResponse {
    element: Element,
    height: u64,
    root_hash: Element,
    txn_hash: CryptoHash,
}

#[tracing::instrument(err, skip_all)]
pub async fn get_element(
    state: web::Data<State>,
    path: web::Path<(Element,)>,
) -> HttpResult<web::Json<ElementResponse>> {
    let (element,) = path.into_inner();
    Ok(web::Json(get_element_response(&state, element)?))
}

#[derive(Deserialize)]
pub struct ListElementsQuery {
    elements: String,
}

#[tracing::instrument(err, skip_all)]
pub async fn list_elements(
    state: web::Data<State>,
    query: web::Query<ListElementsQuery>,
) -> HttpResult<web::Json<Vec<ElementResponse>>> {
    if query.elements.is_empty() {
        return Ok(web::Json(vec![]));
    }

    let elements = query
        .elements
        .split(',')
        .map(|c| {
            Element::from_str(c)
                .map_err(|e| error::Error::InvalidElement(c.to_string(), e))
                .map_err(rpc::error::HTTPError::from)
        })
        .collect::<HttpResult<Vec<Element>>>()?;

    Ok(web::Json(
        elements
            .iter()
            .map(|element| match get_element_response(&state, *element) {
                Ok(response) => Ok(Some(response)),
                Err(e) => {
                    if let crate::Error::ElementNotInTree { .. } = e {
                        Ok(None)
                    } else {
                        Err(e)
                    }
                }
            })
            .filter_map(Result::transpose)
            .collect::<crate::Result<Vec<ElementResponse>>>()?,
    ))
}

fn get_element_response(
    state: &web::Data<State>,
    element: Element,
) -> crate::Result<ElementResponse> {
    let notes_tree = state.node.notes_tree().read();
    let tree = notes_tree.tree();
    let meta = tree
        .get(element)
        .ok_or(crate::Error::ElementNotInTree { element })?;

    let Some(block) = state.node.get_block(meta.inserted_in.into())? else {
        return Err(crate::Error::BlockNotFound { block: meta.inserted_in.into() });
    };

    let block = block.into_block();
    let root_hash = block.content.state.root_hash;
    let txn = block
        .content
        .state
        .txns
        .iter()
        .find(|txn| txn.leaves().contains(&element));
    let Some(txn) = txn else {
        // This should never happen in practice
        return Err(crate::Error::ElementNotInTxn { element, block_height: block.block_height() });
    };
    let txn_hash = txn.hash();

    Ok(ElementResponse {
        element,
        height: meta.inserted_in,
        root_hash,
        txn_hash,
    })
}
