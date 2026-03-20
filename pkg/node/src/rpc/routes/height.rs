use super::State;
use actix_web::web;
use rpc::error::HttpResult;
use serde::Serialize;
use zk_primitives::Element;

#[derive(Serialize)]
pub struct HeightResp {
    height: u64,
    root_hash: Element,
}

/// GET /height - returns data about the rollup (e.g. root hash, version, etc)
#[tracing::instrument(err, skip(state))]
pub async fn get_height(state: web::Data<State>) -> HttpResult<web::Json<HeightResp>> {
    Ok(web::Json(HeightResp {
        height: state.node.height().0,
        root_hash: state.node.root_hash(),
    }))
}
